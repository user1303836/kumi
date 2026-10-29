//! Text selected from the displayed cells. Keep that frame while selecting so streaming cannot
//! replace the words under the mouse. A selection stays inside the pane where the drag began.
//! A double click takes a word and a triple click a line. A copy joins the rows the conversation
//! wrapped from one line, and leaves out its margin and the leads its wrapped rows repeat.

use super::screen::{Rect, Screen};

/// How far a press reaches: the cell under it, its word, or its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unit {
    Cell,
    Word,
    Line,
}

/// A conversation row as a frame drew it: where its text starts (past the margin, and on a row that carries on a
/// wrapped line, past the lead it repeats) and ends, and whether it carries on the row above, joined by a space or by
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TextRow {
    pub y: i32,
    pub start: i32,
    pub end: i32,
    pub carries: Option<bool>,
}

/// What a frame says of its cells, kept beside them: its conversation rows, and the areas a drag selects in.
#[derive(Clone, Debug, Default)]
pub(super) struct Marks {
    pub rows: Vec<TextRow>,
    pub areas: Vec<Rect>,
}

impl Marks {
    /// The area a press at (x, y) selects in, if any.
    pub fn area_at(&self, x: i32, y: i32) -> Option<Rect> {
        self.areas.iter().copied().find(|a| x >= a.x && x < a.x + a.width && y >= a.y && y < a.y + a.height)
    }
}

/// A cell, as (row, column).
type Cell = (i32, i32);

pub(super) struct Selection {
    pub screen: Screen,
    /// The frame's marks, all of them: the frame they came with is kept again while this one shows.
    pub marks: Marks,
    area: Rect,
    /// The frame's conversation rows inside `area`, top to bottom.
    rows: Vec<TextRow>,
    unit: Unit,
    /// What the first press took and what the drag reaches (a cell, a word, a line), each from its first cell to its
    /// last.
    anchor: (Cell, Cell),
    end: (Cell, Cell),
    pub dragging: bool,
    pub moved: bool,
}

impl Selection {
    /// A press at (x, y) on a frame with these marks, inside the area it falls in, taking `unit`: None outside every
    /// area, or for a word where there's none.
    pub fn new(screen: Screen, marks: Marks, x: i32, y: i32, unit: Unit) -> Option<Self> {
        let area = marks.area_at(x, y)?;
        let inside = |row: &&TextRow| (area.y..area.y + area.height).contains(&row.y) && (area.x..area.x + area.width).contains(&row.start);
        let rows = marks.rows.iter().filter(inside).copied().collect();
        let mut selection = Self {
            screen,
            marks,
            area,
            rows,
            unit,
            anchor: ((0, 0), (0, 0)),
            end: ((0, 0), (0, 0)),
            dragging: true,
            moved: unit != Unit::Cell,
        };
        selection.anchor = selection.reach(selection.point(x, y))?;
        selection.end = selection.anchor;
        Some(selection)
    }

    /// A press again at (x, y) on the frame this selection holds: a double click becoming a triple.
    pub fn again(self, x: i32, y: i32, unit: Unit) -> Option<Self> {
        Self::new(self.screen, self.marks, x, y, unit)
    }

    fn point(&self, x: i32, y: i32) -> Cell {
        let y = y.clamp(self.area.y, self.area.y + self.area.height - 1);
        let mut x = x.clamp(self.area.x, self.area.x + self.area.width - 1);
        if x > self.area.x && self.width_at((y, x)) == 0 {
            x -= 1;
        }
        (y, x)
    }

    pub fn update(&mut self, x: i32, y: i32) {
        let point = self.point(x, y);
        // Dragged on past the words onto blanks, a drag by words reaches the cell.
        self.end = self.reach(point).unwrap_or((point, point));
        self.moved |= self.end != self.anchor;
    }

    fn reach(&self, at: Cell) -> Option<(Cell, Cell)> {
        match self.unit {
            Unit::Cell => Some((at, at)),
            Unit::Word => self.word(at),
            Unit::Line => Some(self.line(at.0)),
        }
    }

    /// The word at a cell: a run of letters and digits in any script, with the marks names, paths and links are made
    /// of, going on across a word the conversation split between rows. A full stop, colon or question mark at its end
    /// is the sentence's. Any other character is a word of its own, and a blank is none.
    fn word(&self, at: Cell) -> Option<(Cell, Cell)> {
        let text = self.char_at(at);
        if text.trim().is_empty() {
            return None;
        }
        if !wordy(&text) {
            return Some((at, at));
        }
        let mut first = at;
        while let Some(before) = self.step(first, false).filter(|cell| wordy(&self.char_at(*cell))) {
            first = before;
        }
        let mut last = at;
        while let Some(after) = self.step(last, true).filter(|cell| wordy(&self.char_at(*cell))) {
            last = after;
        }
        while last != first && matches!(self.char_at(last).as_str(), "." | ":" | "?") {
            last = self.step(last, false).unwrap_or(first);
        }
        Some((first, last))
    }

    /// The line at a row: a conversation row with the rows wrapped from the same line, from its text's first cell to
    /// the last of its row's (a step's time too); any other row on its own.
    fn line(&self, y: i32) -> (Cell, Cell) {
        let mut first = y;
        while self.row(first).is_some_and(|row| row.carries.is_some()) && self.row(first - 1).is_some() {
            first -= 1;
        }
        let mut last = y;
        while self.row(last + 1).is_some_and(|row| row.carries.is_some()) {
            last += 1;
        }
        ((first, self.start(first)), (last, self.area.x + self.area.width - 1))
    }

    /// The next character's cell along a row, or the one before: past a row's text only into a row that carries it on
    /// with nothing between, a word the conversation split between rows.
    fn step(&self, (y, x): Cell, forward: bool) -> Option<Cell> {
        let right = self.area.x + self.area.width;
        if forward {
            let next = x + i32::from(self.width_at((y, x)).max(1));
            let split = self.row(y).is_some_and(|row| next == row.end);
            if let Some(below) = self.row(y + 1).filter(|row| split && row.carries == Some(false) && row.start < row.end) {
                return Some((y + 1, below.start));
            }
            return (next < right).then_some((y, next));
        }
        if let Some(before) = self.before(y, x, self.start(y)) {
            return Some((y, before));
        }
        self.row(y).filter(|row| row.carries == Some(false))?;
        let above = self.row(y - 1)?;
        self.before(y - 1, above.end, above.start).map(|before| (y - 1, before))
    }

    /// The character before column `x` on row `y`, by its first cell, if it isn't before `start`.
    fn before(&self, y: i32, x: i32, start: i32) -> Option<i32> {
        let mut before = x - 1;
        if before > start && self.width_at((y, before)) == 0 {
            before -= 1;
        }
        (before >= start).then_some(before)
    }

    /// The frame's conversation row at `y`.
    fn row(&self, y: i32) -> Option<&TextRow> {
        self.rows.binary_search_by_key(&y, |row| row.y).ok().map(|index| &self.rows[index])
    }

    /// Where a row's text starts: a conversation row's own start, any other row's at the area's edge.
    fn start(&self, y: i32) -> i32 {
        self.row(y).map_or(self.area.x, |row| row.start.min(self.area.x + self.area.width - 1))
    }

    fn index(&self, (y, x): Cell) -> usize {
        (y * self.screen.width + x) as usize
    }

    fn char_at(&self, cell: Cell) -> String {
        let mut text = String::new();
        self.screen.push_grapheme(self.index(cell), &mut text);
        text
    }

    fn width_at(&self, cell: Cell) -> u8 {
        self.screen.widths[self.index(cell)]
    }

    fn spans(&self) -> Vec<(i32, i32, i32)> {
        let (first, last) = (self.anchor.0.min(self.end.0), self.anchor.1.max(self.end.1));
        (first.0..=last.0)
            .map(|y| {
                let left = if y == first.0 { first.1 } else { self.area.x };
                let right = if y == last.0 { last.1 + i32::from(self.width_at((y, last.1)).max(1)) } else { self.area.x + self.area.width };
                (y, left, right.min(self.area.x + self.area.width))
            })
            .collect()
    }

    /// The selected text, a line for each row. Rows the conversation wrapped from one line are joined as the line
    /// was, and its margin and the leads of wrapped rows are left out.
    pub fn text(&self) -> String {
        let spans = self.spans();
        let mut text = String::new();
        for (index, &(y, left, right)) in spans.iter().enumerate() {
            let row = self.row(y);
            // A row the next one carries on is taken to its text's end, so the spaces where code was split stay; only
            // blanks after it go.
            let carried = spans.get(index + 1).and_then(|&(below, ..)| self.row(below)).is_some_and(|below| below.carries.is_some());
            let keep = row.filter(|_| carried).map_or(left, |row| row.end);
            let left = row.map_or(left, |row| left.max(row.start));
            let (mut line, mut kept) = (String::new(), 0);
            for x in left..right {
                let at = self.index((y, x));
                if self.screen.widths[at] != 0 {
                    self.screen.push_grapheme(at, &mut line);
                }
                if x < keep {
                    kept = line.len();
                }
            }
            line.truncate(line.trim_end().len().max(kept));
            if index > 0 {
                match row.and_then(|row| row.carries) {
                    Some(true) if !text.is_empty() => text.push(' '),
                    Some(_) => {}
                    None => text.push('\n'),
                }
            }
            text.push_str(&line);
        }
        // Blank rows a drag ran over before or after the text aren't copied.
        text.truncate(text.trim_end().len());
        text.trim_start_matches('\n').to_string()
    }

    pub fn painted(&self) -> Screen {
        let mut screen = self.screen.clone();
        if self.moved {
            for (y, left, right) in self.spans() {
                for x in left..right {
                    let at = (y * screen.width + x) as usize;
                    let mut style = screen.table.style(screen.styles[at]);
                    style.inverse = !style.inverse;
                    screen.styles[at] = screen.table.id(&style);
                }
            }
        }
        screen
    }
}

/// Whether a cell's character belongs in a word: a letter or digit in any script, or one of the marks names, paths and
/// links are made of.
fn wordy(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_alphanumeric() || "_-./~@+%#&=?:".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::style::Style;

    fn screen(width: i32, rows: &[(i32, i32, &str)]) -> Screen {
        let mut screen = Screen::new(width, rows.iter().map(|(_, y, _)| y + 1).max().unwrap_or(1));
        for (x, y, text) in rows {
            screen.put(*x, *y, text, &Style::default());
        }
        screen
    }

    fn row(y: i32, start: i32, end: i32, carries: Option<bool>) -> TextRow {
        TextRow { y, start, end, carries }
    }

    fn marks(rows: &[TextRow], area: Rect) -> Marks {
        Marks { rows: rows.to_vec(), areas: vec![area] }
    }

    #[test]
    fn reverse_drag_keeps_unicode_and_excludes_the_other_pane() {
        let mut screen = Screen::new(20, 3);
        screen.put(0, 0, "A界e\u{301}  ", &Style::default());
        screen.put(0, 1, "second", &Style::default());
        screen.put(10, 0, "OTHER", &Style::default());
        let mut selection = Selection::new(screen, marks(&[], Rect::new(0, 0, 8, 3)), 5, 1, Unit::Cell).unwrap();
        selection.update(2, 0); // continuation cell of 界
        assert_eq!(selection.text(), "界e\u{301}\nsecond");
        let painted = selection.painted();
        assert!(painted.table.style(painted.styles[1]).inverse);
        assert!(painted.table.style(painted.styles[2]).inverse);
        assert!(!painted.table.style(painted.styles[10]).inverse);
    }

    #[test]
    fn a_drag_outside_the_screen_clamps_to_the_original_pane() {
        let mut screen = Screen::new(10, 2);
        screen.put(5, 0, "right", &Style::default());
        screen.put(5, 1, "end", &Style::default());
        let mut selection = Selection::new(screen, marks(&[], Rect::new(5, 0, 5, 2)), 5, 0, Unit::Cell).unwrap();
        selection.update(500, 500);
        assert_eq!(selection.text(), "right\nend");
    }

    #[test]
    fn a_word_is_letters_digits_and_the_marks_of_names_paths_and_links() {
        let screen =
            screen(60, &[(0, 0, "Load Kick_01_final, then -6.5 dB."), (0, 1, "See https://example.com/a?b=1. Or ~/Music/kit.wav?")]);
        let word = |x, y| Selection::new(screen.clone(), marks(&[], Rect::new(0, 0, 60, 2)), x, y, Unit::Word).map(|s| s.text());
        assert_eq!(word(7, 0).as_deref(), Some("Kick_01_final"));
        assert_eq!(word(26, 0).as_deref(), Some("-6.5"));
        // The sentence's full stop, colon or question mark isn't the word's; a link's own marks are.
        assert_eq!(word(31, 0).as_deref(), Some("dB"));
        assert_eq!(word(10, 1).as_deref(), Some("https://example.com/a?b=1"));
        assert_eq!(word(40, 1).as_deref(), Some("~/Music/kit.wav"));
        // A comma is a word of its own; a blank is none.
        assert_eq!(word(18, 0).as_deref(), Some(","));
        assert_eq!(word(4, 0), None);
        assert_eq!(word(55, 0), None);
    }

    #[test]
    fn a_word_in_wide_characters_and_combining_marks_is_whole() {
        let screen = screen(30, &[(0, 0, "主旋律と e\u{301}cole 🎹 x")]);
        let word = |x| Selection::new(screen.clone(), marks(&[], Rect::new(0, 0, 30, 1)), x, 0, Unit::Word).unwrap().text();
        // A press on a wide character's second cell takes its word.
        assert_eq!(word(3), "主旋律と");
        assert_eq!(word(9), "e\u{301}cole");
        assert_eq!(word(15), "🎹");
    }

    #[test]
    fn a_word_goes_on_across_rows_split_inside_it_but_not_past_a_space() {
        // A link the conversation split between characters, then a line wrapped between words.
        let screen = screen(20, &[(3, 0, "https://exampl"), (3, 1, "e.com/x then"), (3, 2, "more")]);
        let rows = [row(0, 3, 17, None), row(1, 3, 15, Some(false)), row(2, 3, 7, Some(true))];
        let word = |x, y| Selection::new(screen.clone(), marks(&rows, Rect::new(0, 0, 20, 3)), x, y, Unit::Word).unwrap().text();
        assert_eq!(word(5, 0), "https://example.com/x");
        assert_eq!(word(4, 1), "https://example.com/x");
        assert_eq!(word(4, 2), "more");
    }

    #[test]
    fn a_line_is_every_row_wrapped_from_it_without_margin_or_leads() {
        // A list item wrapped under its marker, a quote under its bar, then a row of its own.
        let screen = screen(
            30,
            &[(3, 0, "• The low end sits"), (3, 1, "  around 120 Hz."), (3, 2, "│ Quoted words"), (3, 3, "│ go on"), (3, 4, "Next")],
        );
        let rows = [row(0, 3, 21, None), row(1, 5, 19, Some(true)), row(2, 3, 17, None), row(3, 5, 10, Some(true)), row(4, 3, 7, None)];
        let line = |y| Selection::new(screen.clone(), marks(&rows, Rect::new(0, 0, 30, 5)), 10, y, Unit::Line).unwrap().text();
        assert_eq!(line(0), "• The low end sits around 120 Hz.");
        assert_eq!(line(1), "• The low end sits around 120 Hz.");
        assert_eq!(line(3), "│ Quoted words go on");
        assert_eq!(line(4), "Next");
        // A drag over all of it, from the margin, keeps the line breaks between lines.
        let mut drag = Selection::new(screen.clone(), marks(&rows, Rect::new(0, 0, 30, 5)), 0, 0, Unit::Cell).unwrap();
        drag.update(29, 4);
        assert_eq!(drag.text(), "• The low end sits around 120 Hz.\n│ Quoted words go on\nNext");
    }

    #[test]
    fn split_code_joins_with_its_own_spaces_and_a_step_keeps_its_time() {
        // A code line split between characters right after a space, and a step row with its time at the right.
        let screen = screen(24, &[(3, 0, "let a = "), (3, 1, "b;"), (3, 2, "│ ✓ looked"), (18, 2, "2.1s")]);
        let rows = [row(0, 3, 11, None), row(1, 3, 5, Some(false)), row(2, 3, 13, None)];
        let mut selection = Selection::new(screen, marks(&rows, Rect::new(0, 0, 24, 3)), 3, 0, Unit::Cell).unwrap();
        selection.update(23, 2);
        assert_eq!(selection.text(), "let a = b;\n│ ✓ looked     2.1s");
    }

    #[test]
    fn a_word_or_a_line_on_a_step_row_reaches_its_time() {
        let screen = screen(24, &[(3, 0, "│ ✓ looked"), (18, 0, "12.3s")]);
        let rows = [row(0, 3, 13, None)];
        let take = |x, unit| Selection::new(screen.clone(), marks(&rows, Rect::new(0, 0, 24, 1)), x, 0, unit).unwrap().text();
        assert_eq!(take(18, Unit::Word), "12.3s");
        assert_eq!(take(22, Unit::Word), "12.3s");
        assert_eq!(take(5, Unit::Line), "│ ✓ looked     12.3s");
    }

    #[test]
    fn a_press_outside_every_area_selects_nothing() {
        let screen = screen(10, &[(0, 0, "words")]);
        assert!(Selection::new(screen, marks(&[], Rect::new(0, 0, 4, 1)), 6, 0, Unit::Cell).is_none());
    }

    #[test]
    fn a_drag_by_words_keeps_whole_words_both_ways() {
        let screen = screen(30, &[(0, 0, "one two three four")]);
        let mut selection = Selection::new(screen, marks(&[], Rect::new(0, 0, 30, 1)), 9, 0, Unit::Word).unwrap();
        assert!(selection.moved);
        selection.update(15, 0);
        assert_eq!(selection.text(), "three four");
        selection.update(1, 0);
        assert_eq!(selection.text(), "one two three");
    }
}
