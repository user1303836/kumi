//! The input box: text and cursor. Positions count graphemes, so the cursor never splits a character.

use super::width::{cell_width, graphemes};

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

#[derive(Clone, Debug, Default)]
pub struct Editor {
    chars: Vec<String>,
    position: usize,
}

fn is_space(grapheme: &str) -> bool {
    grapheme.chars().any(char::is_whitespace)
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

    pub fn set(&mut self, text: &str) {
        self.chars = graphemes(text).into_iter().map(str::to_string).collect();
        self.position = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.position = 0;
    }

    pub fn insert(&mut self, text: &str) {
        let before = self.chars[..self.position].concat() + text;
        // Re-segment, so a combining mark typed after a letter joins it.
        let after = self.chars[self.position..].concat();
        self.chars = graphemes(&(before.clone() + &after)).into_iter().map(str::to_string).collect();
        self.position = graphemes(&before).len();
    }

    pub fn backspace(&mut self) {
        if self.position == 0 {
            return;
        }
        self.chars.remove(self.position - 1);
        self.position -= 1;
    }

    pub fn delete(&mut self) {
        if self.position < self.chars.len() {
            self.chars.remove(self.position);
        }
    }

    pub fn left(&mut self) {
        self.position = self.position.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.position = (self.position + 1).min(self.chars.len());
    }

    pub fn word_left(&mut self) {
        while self.position > 0 && is_space(&self.chars[self.position - 1]) {
            self.position -= 1;
        }
        while self.position > 0 && !is_space(&self.chars[self.position - 1]) {
            self.position -= 1;
        }
    }

    pub fn word_right(&mut self) {
        while self.position < self.chars.len() && is_space(&self.chars[self.position]) {
            self.position += 1;
        }
        while self.position < self.chars.len() && !is_space(&self.chars[self.position]) {
            self.position += 1;
        }
    }

    pub fn delete_word_left(&mut self) {
        let end = self.position;
        self.word_left();
        self.chars.drain(self.position..end);
    }

    pub fn home(&mut self) {
        while self.position > 0 && self.chars[self.position - 1] != "\n" {
            self.position -= 1;
        }
    }

    pub fn end(&mut self) {
        while self.position < self.chars.len() && self.chars[self.position] != "\n" {
            self.position += 1;
        }
    }

    pub fn kill_to_end(&mut self) {
        let mut end = self.position;
        while end < self.chars.len() && self.chars[end] != "\n" {
            end += 1;
        }
        self.chars.drain(self.position..end);
    }

    pub fn kill_to_start(&mut self) {
        let end = self.position;
        self.home();
        self.chars.drain(self.position..end);
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
        true
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
            let cells = cell_width(character);
            if column + cells > width {
                rows.push(String::new());
                column = 0;
            }
            places.push(Place { row: rows.len() - 1, column });
            rows.last_mut().expect("a row").push_str(character);
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
