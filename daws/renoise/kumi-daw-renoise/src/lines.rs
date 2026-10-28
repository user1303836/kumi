//! A pattern track's lines, or a phrase's, as Renoise's variations work on them (M7): each line's note cells and effect cells in
//! Renoise's own text (read from the tool's rows, written back as notes_write and phrase_write cells), what sounds where (for
//! the distance between two takes), and the variation operators: each seeded, each leaving the anchor lines alone.
use kumi_agent::core::variations::{Material, Rng};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// A note column's cell. `note` is Renoise's text ("C-4", "OFF", "---"); the instrument (a pattern's instrument or a phrase's
/// sample, from 00) and the delay are numbers; volume and panning keep their two characters (a level or a column command).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    pub col: u8,
    pub note: String,
    pub instrument: Option<u8>,
    pub volume: Option<String>,
    pub panning: Option<String>,
    pub delay: u8,
    /// The column's own effect: two characters and a hex amount ("0S40").
    pub fx: Option<String>,
}

/// An effect column's cell: its number ("0S") and amount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub col: u8,
    pub number: String,
    pub amount: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub notes: Vec<Note>,
    pub effects: Vec<Effect>,
}

impl Line {
    fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.effects.is_empty()
    }

    /// The note cells that start a sound (not OFF, not an empty note).
    fn hit_cols(&self) -> Vec<u8> {
        self.notes.iter().filter(|note| is_note(&note.note)).map(|note| note.col).collect()
    }

    fn effect(&self, number: &str) -> Option<&Effect> {
        self.effects.iter().find(|effect| effect.number == number)
    }

    /// Sets (or adds in the first free column) an effect; removes it with None.
    fn set_effect(&mut self, number: &str, amount: Option<u8>) {
        match amount {
            None => self.effects.retain(|effect| effect.number != number),
            Some(amount) => {
                if let Some(effect) = self.effects.iter_mut().find(|effect| effect.number == number) {
                    effect.amount = amount;
                    return;
                }
                let used: BTreeSet<u8> = self.effects.iter().map(|effect| effect.col).collect();
                if let Some(col) = (1..=8u8).find(|col| !used.contains(col)) {
                    self.effects.push(Effect { col, number: number.into(), amount });
                    self.effects.sort_by_key(|effect| effect.col);
                }
            }
        }
    }
}

/// Lines of a range (a pattern track's first..last, or a phrase's lines), numbered from 0 within it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lines {
    pub length: u32,
    pub lpb: u32,
    pub rows: BTreeMap<u32, Line>,
    /// Lines (from 0 in the range) every take keeps as they are: what the model says defines the idea.
    pub anchors: BTreeSet<u32>,
}

const NOTE_NAMES: [&str; 12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];

/// Renoise's note number of a note's text (C-4 is 48), or None for OFF, an empty note or text that isn't a note.
pub fn note_value(text: &str) -> Option<u8> {
    let text = text.to_ascii_uppercase();
    if text.len() != 3 {
        return None;
    }
    let (name, octave) = text.split_at(2);
    let index = NOTE_NAMES.iter().position(|n| *n == name)?;
    let octave: u8 = octave.parse().ok()?;
    let value = octave as u32 * 12 + index as u32;
    (value <= 119).then_some(value as u8)
}

pub fn note_text(value: u8) -> String {
    format!("{}{}", NOTE_NAMES[(value % 12) as usize], value / 12)
}

fn is_note(text: &str) -> bool {
    note_value(text).is_some()
}

fn hex_byte(value: Option<&Value>) -> Option<u8> {
    match value? {
        Value::String(text) if text != ".." => u8::from_str_radix(text, 16).ok(),
        Value::Number(n) => n.as_u64().and_then(|n| u8::try_from(n).ok()),
        _ => None,
    }
}

fn text_field(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).filter(|text| !text.is_empty() && *text != "..").map(str::to_ascii_uppercase)
}

/// A level in the volume column, 0 to 128 (a column command or none is full).
fn level(volume: &Option<String>) -> u8 {
    volume.as_deref().and_then(|text| u8::from_str_radix(text, 16).ok()).filter(|v| *v <= 0x80).unwrap_or(0x80)
}

fn digest_text(text: &str) -> String {
    // (FNV-1a: what Kumi wrote, told from what's there now within one run)
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Lines {
    /// The lines of the tool's rows (each `{line, notes?, effects?, hidden?}`, numbered as Renoise shows them) that fall in
    /// first..=last, numbered from 0 in that range. Hidden columns' cells are kept with the rest.
    pub fn from_rows(rows: &[Value], first: u32, last: u32, lpb: u32) -> Lines {
        let mut out = BTreeMap::new();
        for row in rows {
            let Some(number) = row.get("line").and_then(Value::as_u64).map(|n| n as u32) else { continue };
            if number < first || number > last {
                continue;
            }
            let mut line = Line::default();
            let lists = [row.get("notes"), row.pointer("/hidden/notes")];
            for cell in lists.into_iter().flatten().filter_map(Value::as_array).flatten() {
                let Some(col) = cell.get("col").and_then(Value::as_u64) else { continue };
                line.notes.push(Note {
                    col: col as u8,
                    note: text_field(cell.get("note")).unwrap_or_else(|| "---".into()),
                    instrument: hex_byte(cell.get("instrument")),
                    volume: text_field(cell.get("volume")),
                    panning: text_field(cell.get("panning")),
                    delay: hex_byte(cell.get("delay")).unwrap_or(0),
                    fx: text_field(cell.get("fx")).filter(|fx| fx != "0000"),
                });
            }
            let lists = [row.get("effects"), row.pointer("/hidden/effects")];
            for cell in lists.into_iter().flatten().filter_map(Value::as_array).flatten() {
                let (Some(col), Some(number)) = (cell.get("col").and_then(Value::as_u64), text_field(cell.get("number"))) else { continue };
                line.effects.push(Effect { col: col as u8, number, amount: hex_byte(cell.get("amount")).unwrap_or(0) });
            }
            line.notes.sort_by_key(|note| note.col);
            line.effects.sort_by_key(|effect| effect.col);
            if !line.is_empty() {
                out.insert(number - first, line);
            }
        }
        Lines { length: last - first + 1, lpb: lpb.max(1), rows: out, anchors: BTreeSet::new() }
    }

    /// The lines the model wrote itself (as notes_write takes them, numbered from `first` as Renoise shows them), as lines.
    pub fn from_cells(cells: &[Value], first: u32, length: u32, lpb: u32) -> Result<Lines, String> {
        for entry in cells {
            let number = entry.get("line").and_then(Value::as_u64).ok_or("each line needs its line number")? as u32;
            if number < first || number >= first + length {
                return Err(format!("line {number:02} is outside the lines varied ({first:02} to {:02})", first + length - 1));
            }
        }
        Ok(Lines::from_rows(cells, first, first + length - 1, lpb))
    }

    /// What it holds, told from anything else it could hold (within one run of Kumi).
    pub fn digest(&self) -> String {
        digest_text(&format!("{}|{}|{}", self.length, self.lpb, stringify_rows(&self.rows, 0)))
    }

    /// Its content as notes_write or phrase_write lines, numbered from `offset` (fresh lines: only those with something).
    pub fn cells(&self, offset: u32) -> Vec<Value> {
        self.rows.iter().map(|(number, line)| cell_json(number + offset, line, false)).collect()
    }

    /// Its content written over `before`'s: every line either has something on, each cleared first, numbered from `offset`.
    pub fn rewrite(&self, before: &Lines, offset: u32) -> Vec<Value> {
        let numbers: BTreeSet<u32> = self.rows.keys().chain(before.rows.keys()).copied().collect();
        numbers
            .into_iter()
            .filter(|number| self.rows.get(number) != before.rows.get(number))
            .map(|number| match self.rows.get(&number) {
                Some(line) => cell_json(number + offset, line, true),
                None => json!({"line": number + offset, "clear": true}),
            })
            .collect()
    }

    /// (#407) The lines `db` quieter by the volume column: each level on a line that isn't an anchor scaled by the gain, and a
    /// note that starts a sound without one (at full) given one; the anchors stay as they are (every take keeps them), and so do
    /// volume column commands. Why not, when nothing is left to turn down.
    pub fn quieter(&self, db: f64) -> Result<Lines, String> {
        let gain = 10f64.powf(-db / 20.);
        let mut out = self.clone();
        let mut turned = 0;
        for (_, line) in out.rows.iter_mut().filter(|(number, _)| !self.anchors.contains(number)) {
            for note in &mut line.notes {
                let level = match note.volume.as_deref().map(|text| u8::from_str_radix(text, 16).ok().filter(|v| *v <= 0x80)) {
                    Some(Some(level)) => level,
                    None if is_note(&note.note) => 0x80,
                    // (a command, or a cell that starts nothing)
                    _ => continue,
                };
                note.volume = Some(format!("{:02X}", (level as f64 * gain).round() as u8));
                turned += 1;
            }
        }
        if turned == 0 {
            return Err("its notes are its anchors, or their volume columns hold commands".into());
        }
        Ok(out)
    }

    /// (#401) Each note cell that starts a sound: its line, its column, where it starts (lines, its delay included), its note
    /// value and its velocity (its volume column's level, 0 to 1: full without one).
    pub fn note_cells(&self) -> Vec<(u32, u8, f64, u8, f64)> {
        let mut out = Vec::new();
        for (number, line) in &self.rows {
            for note in line.notes.iter().filter(|note| is_note(&note.note)) {
                let at = *number as f64 + note.delay as f64 / 256.;
                out.push((*number, note.col, at, note_value(&note.note).unwrap_or(0), level(&note.volume) as f64 / 128.));
            }
        }
        out
    }

    /// Each sound's start (256ths of a line from the range's start) and what it plays.
    fn hits(&self) -> Vec<Hit> {
        let mut out = Vec::new();
        for (number, line) in &self.rows {
            let offset = line.effect("0S").map(|effect| effect.amount);
            let reverse = line.effect("0B").is_some_and(|effect| effect.amount == 0);
            let retrig = line.effect("0R").map(|effect| effect.amount);
            for note in line.notes.iter().filter(|note| is_note(&note.note)) {
                let fx = note.fx.as_deref().unwrap_or("");
                let fx_amount = fx.get(2..4).and_then(|amount| u8::from_str_radix(amount, 16).ok());
                out.push(Hit {
                    at: number * 256 + note.delay as u32,
                    note: note_value(&note.note).unwrap_or(0),
                    instrument: note.instrument,
                    loud: level(&note.volume) / 16,
                    offset: if fx.starts_with("0S") { fx_amount } else { offset },
                    reverse: reverse || (fx.starts_with("0B") && fx_amount == Some(0)),
                    retrig: if fx.starts_with("0R") { fx_amount } else { retrig },
                });
            }
        }
        out
    }

    /// The anchors' lines as they are, for a take to keep.
    fn anchored(&self) -> BTreeMap<u32, Option<&Line>> {
        self.anchors.iter().map(|number| (*number, self.rows.get(number))).collect()
    }

    /// `source`'s anchor lines put in as they are (an empty one emptied), and its anchors kept: lines the model wrote never
    /// have to repeat them (#400 review notes).
    pub fn keep_anchors_of(&mut self, source: &Lines) {
        for (number, line) in source.anchored() {
            match line {
                Some(line) => {
                    self.rows.insert(number, line.clone());
                }
                None => {
                    self.rows.remove(&number);
                }
            }
        }
        self.anchors = source.anchors.clone();
    }

    /// Whether `take` keeps every anchor line of `self` as it is.
    pub fn anchors_kept_in(&self, take: &Lines) -> Result<(), u32> {
        for (number, line) in self.anchored() {
            if take.rows.get(&number) != line {
                return Err(number);
            }
        }
        Ok(())
    }

    /// The lines with a hit an operator may touch: not anchored, holding exactly one sound (a slice of a chop, a note of a
    /// line), in order.
    fn free_hits(&self) -> Vec<u32> {
        self.rows.iter().filter(|(number, line)| !self.anchors.contains(number) && line.hit_cols().len() == 1).map(|(n, _)| *n).collect()
    }

    fn free_empty(&self) -> Vec<u32> {
        (0..self.length).filter(|number| !self.anchors.contains(number) && !self.rows.contains_key(number)).collect()
    }

    fn beat_of(&self, number: u32) -> u32 {
        number / self.lpb
    }
}

fn stringify_rows(rows: &BTreeMap<u32, Line>, offset: u32) -> String {
    let cells: Vec<Value> = rows.iter().map(|(number, line)| cell_json(number + offset, line, false)).collect();
    kumi_common::js::json::stringify(&Value::Array(cells))
}

fn cell_json(number: u32, line: &Line, clear: bool) -> Value {
    let mut out = Map::new();
    out.insert("line".into(), json!(number));
    if clear {
        out.insert("clear".into(), json!(true));
    }
    let notes: Vec<Value> = line
        .notes
        .iter()
        .map(|note| {
            let mut cell = Map::new();
            cell.insert("col".into(), json!(note.col));
            cell.insert("note".into(), json!(note.note));
            if let Some(instrument) = note.instrument {
                cell.insert("instrument".into(), json!(format!("{instrument:02X}")));
            }
            if let Some(volume) = &note.volume {
                cell.insert("volume".into(), json!(volume));
            }
            if let Some(panning) = &note.panning {
                cell.insert("panning".into(), json!(panning));
            }
            if note.delay > 0 {
                cell.insert("delay".into(), json!(format!("{:02X}", note.delay)));
            }
            if let Some(fx) = &note.fx {
                cell.insert("fx".into(), json!(fx));
            }
            Value::Object(cell)
        })
        .collect();
    let effects: Vec<Value> = line
        .effects
        .iter()
        .map(|effect| json!({"col": effect.col, "number": effect.number, "amount": format!("{:02X}", effect.amount)}))
        .collect();
    if !notes.is_empty() {
        out.insert("notes".into(), Value::Array(notes));
    }
    if !effects.is_empty() {
        out.insert("effects".into(), Value::Array(effects));
    }
    Value::Object(out)
}

/// One sound: where it starts and what plays.
#[derive(Debug, Clone, PartialEq)]
struct Hit {
    at: u32,
    note: u8,
    instrument: Option<u8>,
    loud: u8,
    offset: Option<u8>,
    reverse: bool,
    retrig: Option<u8>,
}

impl Hit {
    /// How different two sounds are, 0 to 1: another note, instrument or sample offset is another sound (a chop's other
    /// slice); played backwards, another roll and another loudness count less, and together at most as much.
    fn differs(&self, other: &Hit) -> f64 {
        let mut differs = 0.;
        if self.note != other.note || self.instrument != other.instrument || self.offset != other.offset {
            differs += 1.;
        }
        if self.reverse != other.reverse {
            differs += 0.6;
        }
        if self.retrig != other.retrig {
            differs += 0.5;
        }
        if self.loud.abs_diff(other.loud) > 2 {
            differs += 0.3;
        }
        f64::min(differs, 1.)
    }
}

/// Starts closer than this (256ths of a line) are the same onset: an eighth of a line.
const SAME_ONSET: u32 = 32;

impl Material for Lines {
    /// Half the share of onsets one has and the other doesn't, half how different the matched sounds are (and another speed
    /// counts as different throughout).
    fn distance(&self, other: &Self) -> f64 {
        let (a, b) = (self.hits(), other.hits());
        if a.is_empty() && b.is_empty() {
            return if self.lpb == other.lpb { 0. } else { 0.5 };
        }
        let mut used = vec![false; b.len()];
        let mut matched = Vec::new();
        for hit in &a {
            let best = b
                .iter()
                .enumerate()
                .filter(|(j, other)| !used[*j] && other.at.abs_diff(hit.at) <= SAME_ONSET)
                .min_by_key(|(_, other)| (other.at.abs_diff(hit.at), (hit.differs(other) * 100.) as u32));
            if let Some((j, other)) = best {
                used[j] = true;
                matched.push(hit.differs(other));
            }
        }
        let onsets = 1. - matched.len() as f64 / a.len().max(b.len()) as f64;
        let content = if matched.is_empty() { 1. } else { matched.iter().sum::<f64>() / matched.len() as f64 };
        let speed = if self.lpb == other.lpb { 0. } else { 0.5 };
        (0.5 * onsets + 0.5 * content).max(speed).min(1.)
    }

    fn silent(&self) -> bool {
        self.hits().is_empty()
    }
}

/// One variation operator and how much of it (0 to 1, about the share of free hits it touches).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Shuffle { amount: f64, bar: bool },
    Retrig { amount: f64, faster: Option<bool> },
    Reverse { amount: f64 },
    Ghosts { amount: f64 },
    Thin { amount: f64 },
    Fill { amount: f64, volume: u8 },
    Accent { amount: f64 },
    Octave { amount: f64, up: Option<bool> },
    Transpose { semitones: i32 },
    Swing { amount: f64 },
    Offsets { amount: f64 },
    Speed { factor: f64 },
}

/// The operators the tool takes, for its description and errors.
pub const OPS: &str = "shuffle (swap slices within a beat; bar: true within a bar), retrig (0Rxy rolls on hits; faster: true/false changes the ones there), reverse (0B00 on hits, or off where there), ghosts (move quiet hits within their beat), thin (drop hits, quiet and off-beat ones first), fill (add quiet copies of the hit before; volume, hex, 20 by default), accent (move loud and quiet among hits), octave (notes an octave up or down; up: true/false), transpose (semitones, every free note), swing (delay the off-beat lines; amount 1 is 96/256), offsets (other 0Sxx sample offsets, from the ones the source uses), speed (a phrase's LPB times factor, 2 or 0.5)";

impl Op {
    pub fn parse(value: &Value) -> Result<Op, String> {
        let name = value.get("op").and_then(Value::as_str).ok_or_else(|| format!("each op names one: {OPS}"))?;
        let amount = value.get("amount").and_then(Value::as_f64).unwrap_or(0.35).clamp(0., 1.);
        let flag = |key: &str| value.get(key).and_then(Value::as_bool);
        Ok(match name {
            "shuffle" => Op::Shuffle { amount, bar: flag("bar").unwrap_or(false) },
            "retrig" => Op::Retrig { amount, faster: flag("faster") },
            "reverse" => Op::Reverse { amount },
            "ghosts" => Op::Ghosts { amount },
            "thin" => Op::Thin { amount },
            "fill" => {
                let volume =
                    value.get("volume").and_then(Value::as_str).and_then(|v| u8::from_str_radix(v, 16).ok()).unwrap_or(0x20).min(0x80);
                Op::Fill { amount, volume }
            }
            "accent" => Op::Accent { amount },
            "octave" => Op::Octave { amount, up: flag("up") },
            "transpose" => {
                let semitones = value.get("semitones").and_then(Value::as_i64).ok_or("transpose takes semitones")?;
                if !(-48..=48).contains(&semitones) {
                    return Err("transpose's semitones go from -48 to 48".into());
                }
                Op::Transpose { semitones: semitones as i32 }
            }
            "swing" => Op::Swing { amount },
            "offsets" => Op::Offsets { amount },
            "speed" => {
                let factor = value.get("factor").and_then(Value::as_f64).unwrap_or(2.);
                if factor != 2. && factor != 0.5 {
                    return Err("speed's factor is 2 (twice as fast) or 0.5 (half as fast)".into());
                }
                Op::Speed { factor }
            }
            other => return Err(format!("{other} isn't an operator: {OPS}")),
        })
    }

    /// The same operator, `factor` times as much of it (an amount stays within 0 to 1; a speed or a transpose is as it was).
    pub fn scaled(&self, factor: f64) -> Op {
        let more = |amount: &f64| (amount * factor).clamp(0.05, 1.);
        match self {
            Op::Shuffle { amount, bar } => Op::Shuffle { amount: more(amount), bar: *bar },
            Op::Retrig { amount, faster } => Op::Retrig { amount: more(amount), faster: *faster },
            Op::Reverse { amount } => Op::Reverse { amount: more(amount) },
            Op::Ghosts { amount } => Op::Ghosts { amount: more(amount) },
            Op::Thin { amount } => Op::Thin { amount: more(amount) },
            Op::Fill { amount, volume } => Op::Fill { amount: more(amount), volume: *volume },
            Op::Accent { amount } => Op::Accent { amount: more(amount) },
            Op::Octave { amount, up } => Op::Octave { amount: more(amount), up: *up },
            Op::Swing { amount } => Op::Swing { amount: more(amount) },
            Op::Offsets { amount } => Op::Offsets { amount: more(amount) },
            Op::Transpose { .. } | Op::Speed { .. } => self.clone(),
        }
    }

    /// Applies itself to `lines` (whose source is `source`, for what the source uses): what it did in a few words, or None
    /// when it found nothing to do.
    pub fn apply(&self, lines: &mut Lines, source: &Lines, rng: &mut Rng) -> Option<String> {
        // (how many of `count` an amount touches: at least one when there is one)
        let some = |count: usize, amount: f64| -> usize {
            if count == 0 {
                return 0;
            }
            ((count as f64 * amount).round() as usize).clamp(1, count)
        };
        match self {
            Op::Shuffle { amount, bar } => {
                // (within a beat; where no beat has two slices to swap, within two beats, then a bar)
                let hits = lines.free_hits();
                let windows = if *bar { vec![lines.lpb * 4] } else { vec![lines.lpb, lines.lpb * 2, lines.lpb * 4] };
                let mut groups: Vec<Vec<u32>> = Vec::new();
                for window in windows {
                    let mut by: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
                    for number in &hits {
                        by.entry(number / window.max(1)).or_default().push(*number);
                    }
                    groups = by.into_values().filter(|group| group.len() >= 2).collect();
                    if !groups.is_empty() {
                        break;
                    }
                }
                let mut swapped = Vec::new();
                for index in rng.choose(groups.len(), some(groups.len(), *amount)) {
                    let group = &groups[index];
                    let (i, j) = (rng.below(group.len()), rng.below(group.len() - 1));
                    let j = if j >= i { j + 1 } else { j };
                    let (a, b) = (group[i.min(j)], group[i.max(j)]);
                    if swap_sounds(lines, a, b) {
                        swapped.push(format!("{a:02} and {b:02}"));
                    }
                }
                (!swapped.is_empty()).then(|| format!("swapped the slices on lines {}", swapped.join(", ")))
            }
            Op::Retrig { amount, faster } => {
                let hits = lines.free_hits();
                let rolled: Vec<u32> = hits.iter().copied().filter(|n| lines.rows[n].effect("0R").is_some()).collect();
                let mut done = Vec::new();
                if let Some(faster) = faster {
                    for number in rolled {
                        let line = lines.rows.get_mut(&number).expect("a line with a hit");
                        let now = line.effect("0R").map(|effect| effect.amount & 0x0F).unwrap_or(3);
                        let to = if *faster { now.saturating_sub(1).max(1) } else { (now + 1).min(11) };
                        if to != now {
                            line.set_effect("0R", Some(to));
                            done.push(format!("{number:02} (0R{to:02X})"));
                        }
                    }
                } else {
                    for index in rng.choose(hits.len(), some(hits.len(), *amount)) {
                        let number = hits[index];
                        let line = lines.rows.get_mut(&number).expect("a line with a hit");
                        let now = line.effect("0R").map(|effect| effect.amount);
                        let choices: Vec<u8> = [2u8, 3, 4, 6].into_iter().filter(|rate| Some(*rate) != now).collect();
                        let rate = choices[rng.below(choices.len())];
                        line.set_effect("0R", Some(rate));
                        done.push(format!("{number:02} (0R{rate:02X})"));
                    }
                }
                (!done.is_empty()).then(|| format!("rolls on lines {}", done.join(", ")))
            }
            Op::Reverse { amount } => {
                let hits = lines.free_hits();
                let mut on = Vec::new();
                let mut off = Vec::new();
                for index in rng.choose(hits.len(), some(hits.len(), *amount)) {
                    let number = hits[index];
                    let line = lines.rows.get_mut(&number).expect("a line with a hit");
                    if line.effect("0B").is_some() {
                        line.set_effect("0B", None);
                        off.push(format!("{number:02}"));
                    } else {
                        line.set_effect("0B", Some(0));
                        on.push(format!("{number:02}"));
                    }
                }
                let mut words = Vec::new();
                if !on.is_empty() {
                    words.push(format!("reversed lines {}", on.join(", ")));
                }
                if !off.is_empty() {
                    words.push(format!("forwards again on {}", off.join(", ")));
                }
                (!words.is_empty()).then(|| words.join("; "))
            }
            Op::Ghosts { amount } => {
                let ghosts: Vec<u32> = lines.free_hits().into_iter().filter(|n| level(&lines.rows[n].notes[0].volume) < 0x40).collect();
                let mut moved = Vec::new();
                for index in rng.choose(ghosts.len(), some(ghosts.len(), *amount)) {
                    let from = ghosts[index];
                    let beat = lines.beat_of(from);
                    let free: Vec<u32> = lines.free_empty().into_iter().filter(|n| lines.beat_of(*n) == beat).collect();
                    if free.is_empty() {
                        continue;
                    }
                    let to = free[rng.below(free.len())];
                    let line = lines.rows.remove(&from).expect("a ghost's line");
                    lines.rows.insert(to, line);
                    moved.push(format!("{from:02}→{to:02}"));
                }
                (!moved.is_empty()).then(|| format!("moved ghosts {}", moved.join(", ")))
            }
            Op::Thin { amount } => {
                let mut hits = lines.free_hits();
                // (quiet hits and off-beat lines go first)
                let lpb = lines.lpb;
                hits.sort_by_key(|n| (level(&lines.rows[n].notes[0].volume) >= 0x40, *n % lpb == 0, *n));
                let count = some(hits.len(), *amount).min(hits.len().saturating_sub(1));
                let mut dropped: Vec<u32> = hits.into_iter().take(count).collect();
                dropped.sort_unstable();
                for number in &dropped {
                    lines.rows.remove(number);
                }
                (!dropped.is_empty()).then(|| format!("dropped hits on lines {}", lines_list(&dropped)))
            }
            Op::Fill { amount, volume } => {
                let free = lines.free_empty();
                let mut added = Vec::new();
                for index in rng.choose(free.len(), some(free.len(), *amount)) {
                    let number = free[index];
                    let Some((_, before)) = lines.rows.range(..number).rev().find(|(_, line)| line.hit_cols().len() == 1) else { continue };
                    let mut line = before.clone();
                    line.effects.retain(|effect| effect.number == "0S" || effect.number == "0B");
                    for note in &mut line.notes {
                        note.volume = Some(format!("{volume:02X}"));
                        note.delay = 0;
                    }
                    lines.rows.insert(number, line);
                    added.push(number);
                }
                added.sort_unstable();
                (!added.is_empty()).then(|| format!("ghost hits added on lines {}", lines_list(&added)))
            }
            Op::Accent { amount } => {
                // (a quiet hit and a loud one trade levels; where all are alike, some go quieter or louder)
                let hits = lines.free_hits();
                let volume = |lines: &Lines, n: &u32| level(&lines.rows[n].notes[0].volume);
                let quiet: Vec<u32> = hits.iter().copied().filter(|n| volume(lines, n) < 0x60).collect();
                let loud: Vec<u32> = hits.iter().copied().filter(|n| volume(lines, n) >= 0x60).collect();
                let mut moved = Vec::new();
                if !quiet.is_empty() && !loud.is_empty() {
                    let pairs = some(quiet.len().min(loud.len()), *amount);
                    let (qs, ls) = (rng.choose(quiet.len(), pairs), rng.choose(loud.len(), pairs));
                    for (q, l) in qs.into_iter().zip(ls) {
                        let (a, b) = (quiet[q], loud[l]);
                        let (va, vb) = (lines.rows[&a].notes[0].volume.clone(), lines.rows[&b].notes[0].volume.clone());
                        lines.rows.get_mut(&a).expect("a hit").notes[0].volume = vb;
                        lines.rows.get_mut(&b).expect("a hit").notes[0].volume = va;
                        moved.push(format!("{a:02}/{b:02}"));
                    }
                } else {
                    let to = if quiet.is_empty() { "50" } else { "80" };
                    for index in rng.choose(hits.len(), some(hits.len(), *amount)) {
                        lines.rows.get_mut(&hits[index]).expect("a hit").notes[0].volume = Some(to.into());
                        moved.push(format!("{:02}", hits[index]));
                    }
                }
                (!moved.is_empty()).then(|| format!("accents moved on lines {}", moved.join(", ")))
            }
            Op::Octave { amount, up } => {
                let hits = lines.free_hits();
                let mut moved = Vec::new();
                for index in rng.choose(hits.len(), some(hits.len(), *amount)) {
                    let number = hits[index];
                    let note = &mut lines.rows.get_mut(&number).expect("a hit").notes[0];
                    let Some(value) = note_value(&note.note) else { continue };
                    let up = up.unwrap_or_else(|| rng.chance(0.5));
                    let to = if up { value as i32 + 12 } else { value as i32 - 12 };
                    if (0..=119).contains(&to) {
                        note.note = note_text(to as u8);
                        moved.push(format!("{number:02} {}", if up { "up" } else { "down" }));
                    }
                }
                (!moved.is_empty()).then(|| format!("an octave moved on lines {}", moved.join(", ")))
            }
            Op::Transpose { semitones } => {
                let mut count = 0;
                for (number, line) in lines.rows.iter_mut() {
                    if lines.anchors.contains(number) {
                        continue;
                    }
                    for note in &mut line.notes {
                        if let Some(value) = note_value(&note.note) {
                            let to = (value as i32 + semitones).clamp(0, 119);
                            note.note = note_text(to as u8);
                            count += 1;
                        }
                    }
                }
                (count > 0 && *semitones != 0).then(|| format!("transposed {count} notes by {semitones:+}"))
            }
            Op::Swing { amount } => {
                let delay = (amount * 96.).round() as u8;
                let half = (lines.lpb / 2).max(1);
                let mut count = 0;
                let anchors = lines.anchors.clone();
                for (number, line) in lines.rows.iter_mut() {
                    if anchors.contains(number) || number % half != 0 || (number / half).is_multiple_of(2) {
                        continue;
                    }
                    for note in line.notes.iter_mut().filter(|note| is_note(&note.note)) {
                        note.delay = delay;
                        count += 1;
                    }
                }
                (count > 0).then(|| format!("swing: off-beats {delay}/256 late"))
            }
            Op::Offsets { amount } => {
                let used: Vec<u8> = {
                    let mut set: BTreeSet<u8> =
                        source.rows.values().filter_map(|line| line.effect("0S").map(|effect| effect.amount)).collect();
                    if set.len() < 2 {
                        set.extend((0..16u8).map(|k| k * 0x10));
                    }
                    set.into_iter().collect()
                };
                let hits: Vec<u32> = lines.free_hits().into_iter().filter(|n| lines.rows[n].effect("0S").is_some()).collect();
                let mut done = Vec::new();
                for index in rng.choose(hits.len(), some(hits.len(), *amount)) {
                    let number = hits[index];
                    let line = lines.rows.get_mut(&number).expect("a hit");
                    let now = line.effect("0S").map(|effect| effect.amount);
                    let choices: Vec<u8> = used.iter().copied().filter(|offset| Some(*offset) != now).collect();
                    if choices.is_empty() {
                        continue;
                    }
                    let to = choices[rng.below(choices.len())];
                    line.set_effect("0S", Some(to));
                    done.push(format!("{number:02} (0S{to:02X})"));
                }
                (!done.is_empty()).then(|| format!("new sample offsets on lines {}", done.join(", ")))
            }
            Op::Speed { factor } => {
                let to = ((lines.lpb as f64) * factor).round().clamp(1., 256.) as u32;
                (to != lines.lpb).then(|| {
                    lines.lpb = to;
                    format!("LPB {to} ({})", if *factor > 1. { "twice as fast" } else { "half as fast" })
                })
            }
        }
    }
}

/// Swaps what two single-hit lines play (the note cell and the slice's own commands), leaving where they fall.
fn swap_sounds(lines: &mut Lines, a: u32, b: u32) -> bool {
    let (Some(first), Some(second)) = (lines.rows.get(&a).cloned(), lines.rows.get(&b).cloned()) else { return false };
    if first == second {
        return false;
    }
    let sound = |line: &Line| {
        (line.notes.clone(), line.effects.iter().filter(|e| e.number == "0S" || e.number == "0B").cloned().collect::<Vec<_>>())
    };
    let keep = |line: &Line| line.effects.iter().filter(|e| e.number != "0S" && e.number != "0B").cloned().collect::<Vec<_>>();
    let ((notes_a, slice_a), (notes_b, slice_b)) = (sound(&first), sound(&second));
    let mut new_a = Line { notes: notes_b, effects: keep(&first) };
    let mut new_b = Line { notes: notes_a, effects: keep(&second) };
    // (each keeps its own timing within its line)
    for (note, old) in new_a.notes.iter_mut().zip(&first.notes) {
        note.delay = old.delay;
    }
    for (note, old) in new_b.notes.iter_mut().zip(&second.notes) {
        note.delay = old.delay;
    }
    for effect in slice_b {
        new_a.set_effect(&effect.number, Some(effect.amount));
    }
    for effect in slice_a {
        new_b.set_effect(&effect.number, Some(effect.amount));
    }
    lines.rows.insert(a, new_a);
    lines.rows.insert(b, new_b);
    true
}

fn lines_list(numbers: &[u32]) -> String {
    numbers.iter().map(|n| format!("{n:02}")).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn written_lines_get_the_sources_anchor_lines_as_they_are() {
        let rows = |pairs: &[(u32, &str)]| -> Vec<Value> {
            pairs.iter().map(|(line, note)| json!({"line": line, "notes": [{"col": 1, "note": note, "instrument": "00"}]})).collect()
        };
        let mut source = Lines::from_rows(&rows(&[(0, "C-4"), (4, "D-4")]), 0, 7, 4);
        source.anchors = [0, 2].into_iter().collect();
        let mut written = Lines::from_rows(&rows(&[(0, "E-4"), (2, "F-4"), (6, "G-4")]), 0, 7, 4);
        written.keep_anchors_of(&source);
        assert!(source.anchors_kept_in(&written).is_ok());
        assert_eq!(written.rows.get(&0), source.rows.get(&0), "the source's line 00");
        assert_eq!(written.rows.get(&2), None, "an empty anchor line stays empty");
        assert!(written.rows.contains_key(&6), "the rest as written");
    }

    use super::*;

    /// A chopped break at LPB 4: kick (00), ghost with an offset (03), snare (04), a roll (06), snare (12), anchors 00 and 04.
    fn chop() -> Lines {
        let rows = vec![
            json!({"line": 16, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "00"}]}),
            json!({"line": 19, "notes": [{"col": 1, "note": "C-4", "instrument": "00", "volume": "20"}], "effects": [{"col": 1, "number": "0S", "amount": "40"}]}),
            json!({"line": 20, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "80"}]}),
            json!({"line": 22, "notes": [{"col": 1, "note": "C-4", "instrument": "00", "delay": "40"}], "effects": [{"col": 1, "number": "0S", "amount": "80"}, {"col": 2, "number": "0R", "amount": "03"}]}),
            json!({"line": 28, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "80"}]}),
            json!({"line": 40, "notes": [{"col": 1, "note": "C-4"}]}),
        ];
        let mut lines = Lines::from_rows(&rows, 16, 31, 4);
        lines.anchors = [0, 4].into_iter().collect();
        lines
    }

    #[test]
    fn rows_read_in_renoises_text_come_back_as_the_same_cells() {
        let lines = chop();
        assert_eq!(lines.length, 16);
        assert_eq!(lines.rows.len(), 5, "line 40 is outside the range");
        assert_eq!(lines.rows[&3].notes[0].volume.as_deref(), Some("20"));
        assert_eq!(lines.rows[&6].notes[0].delay, 0x40);
        let cells = lines.cells(16);
        assert_eq!(
            cells[0],
            json!({"line": 16, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "00"}]})
        );
        assert_eq!(cells[3]["notes"][0]["delay"], "40");
        let again = Lines::from_rows(&cells, 16, 31, 4);
        assert_eq!(again.rows, lines.rows, "written and read again, the same");
        assert_eq!(note_value("C-4"), Some(48));
        assert_eq!(note_value("OFF"), None);
        assert_eq!(note_text(61), "C#5");
    }

    #[test]
    fn a_rewrite_clears_what_went_and_writes_what_changed_only() {
        let before = chop();
        let mut after = before.clone();
        after.rows.remove(&3);
        after.rows.get_mut(&12).unwrap().notes[0].note = "D-4".into();
        let cells = after.rewrite(&before, 16);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0], json!({"line": 19, "clear": true}));
        assert_eq!(cells[1]["line"], 28);
        assert_eq!(cells[1]["clear"], true);
        assert_eq!(cells[1]["notes"][0]["note"], "D-4");
    }

    #[test]
    fn distances_count_onsets_and_what_plays_at_them() {
        let source = chop();
        assert_eq!(source.distance(&source), 0.);
        let mut moved = source.clone();
        let line = moved.rows.remove(&12).unwrap();
        moved.rows.insert(13, line);
        let mut reversed = source.clone();
        reversed.rows.get_mut(&12).unwrap().set_effect("0B", Some(0));
        let d_moved = source.distance(&moved);
        let d_reversed = source.distance(&reversed);
        assert!((d_moved - 0.1).abs() < 1e-9, "one onset of five moved: {d_moved}");
        assert!(d_reversed > 0. && d_reversed < d_moved, "a reverse is a smaller change than a moved hit: {d_reversed} {d_moved}");
        let mut faster = source.clone();
        faster.lpb = 8;
        assert!(source.distance(&faster) >= 0.5, "another speed throughout");
        let empty = Lines::from_rows(&[], 0, 15, 4);
        assert!(empty.silent());
        assert_eq!(source.distance(&empty), 1.);
    }

    #[test]
    fn every_operator_is_seeded_and_leaves_the_anchors_as_they_are() {
        let source = chop();
        let ops = [
            json!({"op": "shuffle", "amount": 1}),
            json!({"op": "retrig", "amount": 0.6}),
            json!({"op": "retrig", "faster": true}),
            json!({"op": "reverse", "amount": 0.6}),
            json!({"op": "ghosts", "amount": 1}),
            json!({"op": "thin", "amount": 0.5}),
            json!({"op": "fill", "amount": 0.3}),
            json!({"op": "accent", "amount": 1}),
            json!({"op": "octave", "amount": 1, "up": true}),
            json!({"op": "transpose", "semitones": 3}),
            json!({"op": "swing", "amount": 0.5}),
            json!({"op": "offsets", "amount": 1}),
            json!({"op": "speed", "factor": 2}),
        ];
        for op in ops {
            let parsed = Op::parse(&op).unwrap();
            let run = |seed| {
                let mut take = source.clone();
                let said = parsed.apply(&mut take, &source, &mut Rng::new(seed));
                (take, said)
            };
            let (take, said) = run(11);
            assert_eq!(run(11), (take.clone(), said.clone()), "{op}: the same seed, the same take");
            let said = said.unwrap_or_else(|| panic!("{op} did nothing"));
            assert!(!said.is_empty());
            assert_eq!(source.anchors_kept_in(&take), Ok(()), "{op}: {said}");
            assert_ne!(take, source, "{op}: {said}");
        }
        assert!(Op::parse(&json!({"op": "explode"})).unwrap_err().contains("isn't an operator"));
        assert!(Op::parse(&json!({"op": "speed", "factor": 3})).is_err());
    }

    /// (#407) A take turned down by the volume column: its levels scaled (a note at full given one), its anchors and a volume
    /// column command as they are, and only those lines written again; nothing left to turn down is said.
    #[test]
    fn a_take_turned_down_scales_its_levels_and_leaves_its_anchors_and_commands() {
        let mut take = chop();
        take.rows.get_mut(&12).unwrap().notes[0].volume = Some("G4".into());
        let quieter = take.quieter(6.0).unwrap();
        let volume = |lines: &Lines, line: u32| lines.rows[&line].notes[0].volume.clone();
        assert_eq!(volume(&quieter, 3).as_deref(), Some("10"), "20, half as loud");
        assert_eq!(volume(&quieter, 6).as_deref(), Some("40"), "a note at full, half as loud");
        assert_eq!((volume(&quieter, 0), volume(&quieter, 4)), (None, None), "the anchors as they are");
        assert_eq!(volume(&quieter, 12).as_deref(), Some("G4"), "a command as it is");
        assert_eq!(take.anchors_kept_in(&quieter), Ok(()));
        let written: Vec<Value> = quieter.rewrite(&take, 16).iter().map(|cell| cell["line"].clone()).collect();
        assert_eq!(written, [json!(19), json!(22)], "only the lines turned down");
        assert_ne!(quieter.digest(), take.digest());
        let mut anchored = chop();
        anchored.anchors = anchored.rows.keys().copied().collect();
        assert!(anchored.quieter(3.).unwrap_err().contains("anchors"));
    }

    #[test]
    fn a_thin_never_empties_the_take_and_quiet_off_beat_hits_go_first() {
        let mut source = chop();
        source.anchors.clear();
        let mut take = source.clone();
        let said = Op::Thin { amount: 0.2 }.apply(&mut take, &source, &mut Rng::new(1)).unwrap();
        assert!(said.contains("03"), "the ghost on 03 goes first: {said}");
        let mut all = source.clone();
        Op::Thin { amount: 1. }.apply(&mut all, &source, &mut Rng::new(1));
        assert!(!all.silent(), "one hit stays");
    }
}
