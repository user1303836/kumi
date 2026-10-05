//! A grid of terminal cells that components draw into; the renderer turns it into output.

use std::rc::Rc;

use super::style::{Style, StyleTable};
use super::width::{cell_width, graphemes};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect { x, y, width, height }
    }
}

pub fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    Rect { x, y, width: (right - x).max(0), height: (bottom - y).max(0) }
}

/// `/[\u0000-\u001f\u007f-\u009f]/`
fn has_control(grapheme: &str) -> bool {
    grapheme.chars().any(|character| character < '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&character))
}

/// The character, width and style at a cell; continuation cells have width 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellAt<'a> {
    pub char: &'a str,
    pub width: u8,
    pub style: Style,
}

/// A wide character (CJK, most emoji) fills its own cell plus the next one, which is
/// kept as an empty continuation cell of width 0. Overwriting either half of a wide
/// character blanks the other half, as a terminal would.
#[derive(Clone, Debug)]
pub struct Screen {
    pub width: i32,
    pub height: i32,
    pub table: Rc<StyleTable>,
    pub chars: Vec<String>,
    pub widths: Vec<u8>,
    pub styles: Vec<u32>,
}

impl Screen {
    pub fn new(width: i32, height: i32) -> Screen {
        Screen::with_table(width, height, Rc::new(StyleTable::new()))
    }

    /// A screen whose style ids come from `table`: frames sharing a table can be compared cell by cell.
    pub fn with_table(width: i32, height: i32, table: Rc<StyleTable>) -> Screen {
        let size = (width * height).max(0) as usize;
        Screen { width, height, table, chars: vec![" ".to_string(); size], widths: vec![1; size], styles: vec![0; size] }
    }

    pub fn bounds(&self) -> Rect {
        Rect { x: 0, y: 0, width: self.width, height: self.height }
    }

    /// Paint a rectangle, clipped to the screen, with spaces in `style`.
    pub fn fill(&mut self, rect: Rect, style: &Style) {
        let id = self.table.id(style);
        let area = intersect(rect, self.bounds());
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                self.set(x, y, " ", 1, id);
            }
        }
    }

    /// Write text starting at (x, y). A style without a background keeps the background
    /// already painted underneath. Returns the column after the text.
    pub fn put(&mut self, x: i32, y: i32, text: &str, style: &Style) -> i32 {
        self.put_in(x, y, text, style, self.bounds())
    }

    /// `put`, inside `clip`.
    pub fn put_in(&mut self, x: i32, y: i32, text: &str, style: &Style, clip: Rect) -> i32 {
        let area = intersect(clip, self.bounds());
        let right = area.x + area.width;
        if y < area.y || y >= area.y + area.height {
            return x;
        }
        let mut column = x;
        for grapheme in graphemes(text) {
            let mut character = grapheme;
            let mut cells = cell_width(grapheme);
            if has_control(grapheme) {
                character = " ";
                cells = 1;
            }
            if cells == 0 {
                continue;
            }
            if column >= right {
                return column + cells;
            }
            if column + cells > right || column < area.x {
                // Only part of a wide character is visible: show its visible cell blank.
                let visible = column.max(area.x);
                if visible < right && visible < column + cells {
                    self.cell(visible, y, " ", 1, style);
                }
            } else {
                self.cell(column, y, character, cells, style);
            }
            column += cells;
        }
        column
    }

    /// The character, width and style id at (x, y); continuation cells have width 0.
    pub fn at(&self, x: i32, y: i32) -> CellAt<'_> {
        let index = (y * self.width + x) as usize;
        CellAt { char: &self.chars[index], width: self.widths[index], style: self.table.style(self.styles[index]) }
    }

    /// The visible text of each row; for tests and debugging.
    pub fn lines(&self) -> Vec<String> {
        let mut rows = Vec::new();
        for y in 0..self.height.max(0) {
            let mut row = String::new();
            for x in 0..self.width.max(0) {
                let index = (y * self.width + x) as usize;
                if self.widths[index] != 0 {
                    row.push_str(&self.chars[index]);
                }
            }
            rows.push(row);
        }
        rows
    }

    /// Take another screen's cells (the same size); for tests that build on the last frame.
    pub fn copy_from(&mut self, other: &Screen) {
        self.chars.clone_from(&other.chars);
        self.widths.clone_from(&other.widths);
        self.styles.clone_from(&other.styles);
    }

    fn cell(&mut self, x: i32, y: i32, character: &str, cells: i32, style: &Style) {
        let index = (y * self.width + x) as usize;
        let underneath = self.table.style(self.styles[index]).bg;
        let id = match underneath {
            Some(bg) if style.bg.is_none() => self.table.id(&Style { bg: Some(bg), ..*style }),
            _ => self.table.id(style),
        };
        self.set(x, y, character, cells, id);
        if cells == 2 {
            self.set(x + 1, y, "", 0, id);
        }
    }

    fn set(&mut self, x: i32, y: i32, character: &str, cells: i32, id: u32) {
        let index = (y * self.width + x) as usize;
        if cells != 0 && self.widths[index] == 0 && x > 0 && self.widths[index - 1] == 2 {
            self.chars[index - 1].replace_range(.., " ");
            self.widths[index - 1] = 1;
        }
        if cells != 2 && self.widths[index] == 2 && x + 1 < self.width {
            self.chars[index + 1].replace_range(.., " ");
            self.widths[index + 1] = 1;
        }
        if self.chars[index] != character {
            self.chars[index].replace_range(.., character);
        }
        self.widths[index] = cells as u8;
        self.styles[index] = id;
    }
}
