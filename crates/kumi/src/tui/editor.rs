//! The input box: text and cursor, the steps undo takes back, and what the cutting keys cut. Positions count graphemes,
//! so the cursor never splits a character.

use super::width::{cell_width, graphemes, text_width};

/// How many steps undo keeps, and how much text they may hold between them (each step keeps the whole text).
const STEPS: usize = 100;
const STEP_BYTES: usize = 4 << 20;
/// How many cuts ctrl+y and alt+y can bring back, and how much text they may hold between them.
const KILLS: usize = 10;
const KILL_BYTES: usize = 4 << 20;
/// A paste longer than this many lines, or characters, goes in as one chip.
const CHIP_LINES: usize = 10;
const CHIP_CHARACTERS: usize = 1000;
/// The character the first chip stands as in the text: Unicode's last private use plane, which pasted text doesn't use
/// (icon fonts take the plane before it).
const CHIP: u32 = 0x10_0000;
/// How much of a chip's first line the conversation shows beside it.
const CHIP_FIRST_LINE: usize = 60;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorLayout {
    pub rows: Vec<String>,
    pub cursor_row: usize,
    pub cursor_column: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    row: usize,
    column: i32,
}

/// The text and cursor before a change: undo puts them back.
#[derive(Clone, Debug)]
struct Step {
    text: String,
    position: usize,
}

/// Changes that grow the newest step while each carries on where the last left the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    Typing,
    Deleting,
}

/// A long paste, kept whole as one character of the box: what was pasted, and what the box shows for it.
#[derive(Clone, Debug)]
struct Chip {
    text: String,
    label: String,
}

/// A draft set aside, with its cursor, its steps and its chips, while the producer looks back through what they sent.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    chars: Vec<String>,
    position: usize,
    steps: Vec<Step>,
    bytes: usize,
    chips: Vec<Chip>,
}

#[derive(Clone, Debug, Default)]
pub struct Editor {
    chars: Vec<String>,
    position: usize,
    /// What undo takes back, newest last, and the bytes of text they hold.
    steps: Vec<Step>,
    bytes: usize,
    /// The run the newest step is still growing, and where its last change left the cursor.
    run: Option<(Run, usize)>,
    /// What the cutting keys cut, newest last.
    kills: Vec<String>,
    /// The last change was a cut, so the next cut joins it, as in the shells: ctrl+w three times, then ctrl+y, puts
    /// back all three words.
    killing: bool,
    /// The cut ctrl+y (or alt+y) has just put in, which alt+y may swap.
    yanked: Option<usize>,
    /// The draft's long pastes, by the character each stands as (`CHIP` onwards): kept while the draft is, since a step
    /// undo takes back may hold one.
    chips: Vec<Chip>,
}

fn is_space(grapheme: &str) -> bool {
    grapheme.chars().any(char::is_whitespace)
}

/// A character a chip stands as: Unicode's last private use plane.
fn is_mark(character: char) -> bool {
    ('\u{100000}'..='\u{10fffd}').contains(&character)
}

/// Text coming into the box, without the characters chips stand as, which typed or pasted text has no use for: any of
/// them in the box is then a chip's own.
fn incoming(text: &str) -> std::borrow::Cow<'_, str> {
    if text.chars().any(is_mark) {
        std::borrow::Cow::Owned(text.chars().filter(|character| !is_mark(*character)).collect())
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
    }

    pub fn text(&self) -> String {
        self.chars.concat()
    }

    pub fn cursor(&self) -> usize {
        self.position
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Replaces the text, as one step, with the cursor at its end.
    pub fn set(&mut self, text: &str) {
        let chars: Vec<String> = graphemes(&incoming(text)).into_iter().map(str::to_string).collect();
        if chars == self.chars {
            self.moved();
        } else {
            self.change(None);
            self.chars = chars;
        }
        self.position = self.chars.len();
    }

    /// Empties the box once its words are sent or held: the next draft starts with nothing to undo. What was cut stays.
    pub fn clear(&mut self) {
        self.chars.clear();
        self.position = 0;
        self.steps.clear();
        self.bytes = 0;
        self.chips.clear();
        self.moved();
    }

    /// A paste: one longer than 10 lines or 1,000 characters goes in as one chip, `[pasted 41 lines]`, which backspace
    /// takes out whole and which is sent as all it holds; a shorter one as its text. One step either way.
    pub fn paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.change(None);
        self.put_pasted(text);
    }

    /// The text with each chip's whole paste in its place: what's sent.
    pub fn expanded(&self) -> String {
        self.whole(&self.chars)
    }

    /// The text with each chip as its label and the start of its first line: what the conversation shows.
    pub fn shown(&self) -> String {
        self.chars
            .iter()
            .map(|grapheme| match self.chip(grapheme) {
                Some((chip, rest)) => {
                    let first = chip.text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
                    let start: String = first.chars().take(CHIP_FIRST_LINE).collect();
                    let more = if start.len() < first.len() { "…" } else { "" };
                    format!("{} {start}{more}{rest}", chip.label)
                }
                None => grapheme.clone(),
            })
            .collect()
    }

    /// Empties the box as a step undo can take back.
    pub fn erase(&mut self) {
        if self.chars.is_empty() {
            return;
        }
        self.change(None);
        self.chars.clear();
        self.position = 0;
    }

    /// Puts a paste, dictated words or a new line in at the cursor: one step each.
    pub fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.change(None);
        self.put(text);
    }

    /// Puts typed text in at the cursor: a run of typing is one step.
    pub fn type_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.change(Some(Run::Typing));
        self.put(text);
        self.run = Some((Run::Typing, self.position));
    }

    /// One character back: a run of deleting is one step.
    pub fn backspace(&mut self) {
        if self.position == 0 {
            return;
        }
        self.change(Some(Run::Deleting));
        self.splice(self.position - 1, self.position, "");
        self.run = Some((Run::Deleting, self.position));
    }

    pub fn delete(&mut self) {
        if self.position == self.chars.len() {
            return;
        }
        self.change(Some(Run::Deleting));
        self.splice(self.position, self.position + 1, "");
        self.run = Some((Run::Deleting, self.position));
    }

    pub fn left(&mut self) {
        self.position = self.position.saturating_sub(1);
        self.moved();
    }

    pub fn right(&mut self) {
        self.position = (self.position + 1).min(self.chars.len());
        self.moved();
    }

    pub fn word_left(&mut self) {
        self.position = self.word_start();
        self.moved();
    }

    pub fn word_right(&mut self) {
        while self.position < self.chars.len() && is_space(&self.chars[self.position]) {
            self.position += 1;
        }
        while self.position < self.chars.len() && !is_space(&self.chars[self.position]) {
            self.position += 1;
        }
        self.moved();
    }

    /// Cuts the word before the cursor.
    pub fn delete_word_left(&mut self) {
        self.kill(self.word_start(), self.position);
    }

    pub fn home(&mut self) {
        self.position = self.line_start();
        self.moved();
    }

    pub fn end(&mut self) {
        self.position = self.line_end();
        self.moved();
    }

    /// Cuts to the end of the line.
    pub fn kill_to_end(&mut self) {
        self.kill(self.position, self.line_end());
    }

    /// Cuts to the start of the line.
    pub fn kill_to_start(&mut self) {
        self.kill(self.line_start(), self.position);
    }

    /// Puts the newest cut back in at the cursor, as a step. False when nothing was cut.
    pub fn yank(&mut self) -> bool {
        let Some(cut) = self.kills.last().cloned() else {
            return false;
        };
        self.change(None);
        self.put_pasted(&cut);
        self.yanked = Some(self.kills.len() - 1);
        true
    }

    /// Right after `yank`, puts the cut before the one it put in in its place, and round to the newest again; still one
    /// step. False at any other time.
    pub fn yank_earlier(&mut self) -> bool {
        let Some(at) = self.yanked.filter(|_| self.kills.len() > 1) else {
            return false;
        };
        // The newest step is the text as it was before the yank.
        let Some(Step { text, position }) = self.steps.last().cloned() else {
            return false;
        };
        let at = (at + self.kills.len() - 1) % self.kills.len();
        self.chars = graphemes(&text).into_iter().map(str::to_string).collect();
        self.position = position;
        let cut = self.kills[at].clone();
        self.put_pasted(&cut);
        self.yanked = Some(at);
        true
    }

    /// Takes back the newest step. False when there's none.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.steps.pop() else {
            return false;
        };
        self.bytes -= step.text.len();
        self.chars = graphemes(&step.text).into_iter().map(str::to_string).collect();
        self.position = step.position;
        self.moved();
        true
    }

    /// Shows an earlier message in the box, with no steps of its own yet. The draft it replaces, steps and all, is
    /// returned, to come back with `restore`.
    pub fn show(&mut self, text: &str) -> Draft {
        let draft = Draft {
            chars: std::mem::replace(&mut self.chars, graphemes(&incoming(text)).into_iter().map(str::to_string).collect()),
            position: self.position,
            steps: std::mem::take(&mut self.steps),
            bytes: std::mem::take(&mut self.bytes),
            chips: std::mem::take(&mut self.chips),
        };
        self.position = self.chars.len();
        self.moved();
        draft
    }

    pub fn restore(&mut self, draft: Draft) {
        self.chars = draft.chars;
        self.position = draft.position;
        self.steps = draft.steps;
        self.bytes = draft.bytes;
        self.chips = draft.chips;
        self.moved();
    }

    /// Rows as displayed at `width` cells (wrapping between characters), with the cursor's place.
    pub fn layout(&self, width: i32) -> EditorLayout {
        self.measure(width.max(1)).0
    }

    /// Move to the row above or below, keeping the column where possible.
    pub fn vertical(&mut self, width: i32, direction: i32) -> bool {
        let (layout, places) = self.measure(width.max(1));
        let row = layout.cursor_row as i64 + direction as i64;
        if row < 0 || row >= layout.rows.len() as i64 {
            return false;
        }
        let row = row as usize;
        let mut best: Option<usize> = None;
        for (index, place) in places.iter().enumerate().take(self.chars.len() + 1) {
            if place.row == row && place.column <= layout.cursor_column {
                best = Some(index);
            }
        }
        let best = best.or_else(|| places.iter().position(|place| place.row == row));
        // TS: findIndex gives -1 when no place is on the row, which can't happen for a row the layout has.
        self.position = best.unwrap_or(0);
        self.moved();
        true
    }

    /// Before each change: the text as it was becomes a step undo takes back, unless this change carries on the run of
    /// typing or deleting that the newest step began, from where its last change left the cursor.
    fn change(&mut self, run: Option<Run>) {
        if run.is_none() || run.map(|run| (run, self.position)) != self.run {
            let step = Step { text: self.chars.concat(), position: self.position };
            self.bytes += step.text.len();
            self.steps.push(step);
            while self.steps.len() > STEPS || (self.bytes > STEP_BYTES && self.steps.len() > 1) {
                self.bytes -= self.steps.remove(0).text.len();
            }
        }
        self.moved();
    }

    /// The cursor moved or the text changed: a run of typing or deleting, a cut's joining and alt+y's turn end here.
    fn moved(&mut self) {
        self.run = None;
        self.killing = false;
        self.yanked = None;
    }

    /// Cuts `start..end`, as a step, into the kill ring, or onto its newest cut right after another cut: after it when
    /// cutting forward, before it when cutting back.
    fn kill(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let joins = self.killing;
        let forward = start == self.position;
        self.change(None);
        // A chip is cut as all it holds: the ring outlives the draft its chips belong to.
        let cut = self.whole(&self.chars[start..end]);
        self.splice(start, end, "");
        match self.kills.last_mut() {
            Some(last) if joins => {
                if forward {
                    last.push_str(&cut);
                } else {
                    last.insert_str(0, &cut);
                }
            }
            _ => self.kills.push(cut),
        }
        while self.kills.len() > KILLS || (self.kills.len() > 1 && self.kills.iter().map(String::len).sum::<usize>() > KILL_BYTES) {
            self.kills.remove(0);
        }
        self.killing = true;
    }

    /// Puts `text` in at the cursor and moves past it.
    fn put(&mut self, text: &str) {
        self.splice(self.position, self.position, &incoming(text));
    }

    /// Puts a paste in at the cursor: as one chip when it's long (see `paste`).
    fn put_pasted(&mut self, text: &str) {
        let (lines, characters) = (text.lines().count(), text.chars().count());
        let mark = char::from_u32(CHIP + self.chips.len() as u32).filter(|mark| (*mark as u32) <= 0x10_fffd);
        match mark.filter(|_| lines > CHIP_LINES || characters > CHIP_CHARACTERS) {
            Some(mark) => {
                let label = if lines > 1 { format!("[pasted {lines} lines]") } else { format!("[pasted {characters} characters]") };
                self.chips.push(Chip { text: text.to_string(), label });
                self.splice(self.position, self.position, &mark.to_string());
            }
            None => self.put(text),
        }
    }

    /// The chip a character of the box stands for, and what was typed after it in the same character (a mark).
    fn chip<'a>(&'a self, grapheme: &'a str) -> Option<(&'a Chip, &'a str)> {
        let first = grapheme.chars().next()?;
        let at = (first as u32).checked_sub(CHIP)? as usize;
        self.chips.get(at).map(|chip| (chip, &grapheme[first.len_utf8()..]))
    }

    /// Characters of the box with each chip's whole paste in its place.
    fn whole(&self, graphemes: &[String]) -> String {
        let mut text = String::new();
        for grapheme in graphemes {
            match self.chip(grapheme) {
                Some((chip, rest)) => {
                    text.push_str(&chip.text);
                    text.push_str(rest);
                }
                None => text.push_str(grapheme),
            }
        }
        text
    }

    /// Replaces characters `from..to` with `text`, leaving the cursor just after it. The text stays split into the
    /// characters a terminal draws, segmenting only around the change: the character before it, which may join what
    /// comes next (a mark typed after a letter), and those after, as far as they change (flags' letters pair up anew to
    /// the end of a run).
    fn splice(&mut self, from: usize, to: usize, text: &str) {
        let start = from.saturating_sub(1);
        let head = self.chars[start..from].concat() + text;
        let mut end = (to + 1).min(self.chars.len());
        loop {
            let window = head.clone() + &self.chars[to..end].concat();
            let pieces: Vec<String> = graphemes(&window).into_iter().map(str::to_string).collect();
            // Settled once the window ends on the very character that was there: what follows it can't change.
            if end == self.chars.len() || pieces.last() == self.chars.get(end - 1) {
                let cursor = start + graphemes(&head).len();
                self.chars.splice(start..end, pieces);
                self.position = cursor;
                return;
            }
            end = (end + (end - to)).min(self.chars.len());
        }
    }

    fn line_start(&self) -> usize {
        let mut at = self.position;
        while at > 0 && self.chars[at - 1] != "\n" {
            at -= 1;
        }
        at
    }

    fn line_end(&self) -> usize {
        let mut at = self.position;
        while at < self.chars.len() && self.chars[at] != "\n" {
            at += 1;
        }
        at
    }

    fn word_start(&self) -> usize {
        let mut at = self.position;
        while at > 0 && is_space(&self.chars[at - 1]) {
            at -= 1;
        }
        while at > 0 && !is_space(&self.chars[at - 1]) {
            at -= 1;
        }
        at
    }

    fn measure(&self, width: i32) -> (EditorLayout, Vec<Place>) {
        let mut rows: Vec<String> = vec![String::new()];
        let mut places: Vec<Place> = Vec::new();
        let mut column = 0;
        for character in &self.chars {
            if character == "\n" {
                places.push(Place { row: rows.len() - 1, column });
                rows.push(String::new());
                column = 0;
                continue;
            }
            // A chip shows as its label.
            let (shown, cells) = match self.chip(character) {
                Some((chip, _)) => (chip.label.as_str(), text_width(&chip.label)),
                None => (character.as_str(), cell_width(character)),
            };
            if column + cells > width {
                rows.push(String::new());
                column = 0;
            }
            places.push(Place { row: rows.len() - 1, column });
            rows.last_mut().expect("a row").push_str(shown);
            column += cells;
        }
        if column >= width {
            rows.push(String::new());
        }
        places.push(Place { row: rows.len() - 1, column: if column >= width { 0 } else { column } });
        let at = places[self.position];
        (EditorLayout { rows, cursor_row: at.row, cursor_column: at.column }, places)
    }
}
