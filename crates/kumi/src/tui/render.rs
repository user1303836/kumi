//! Turns screens into terminal output, writing only the cells that changed since the last frame.

use std::rc::Rc;

use super::screen::Screen;
use super::style::{sgr, ColorDepth, StyleTable};

pub fn cursor_to(x: i32, y: i32) -> String {
    format!("\u{1b}[{};{}H", y + 1, x + 1)
}

const SYNC_START: &str = "\u{1b}[?2026h";
const SYNC_END: &str = "\u{1b}[?2026l";
const HIDE_CURSOR: &str = "\u{1b}[?25l";
const SHOW_CURSOR: &str = "\u{1b}[?25h";
const RESET: &str = "\u{1b}[0m";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub x: i32,
    pub y: i32,
}

struct Frame {
    width: i32,
    height: i32,
    table: Rc<StyleTable>,
    chars: Vec<String>,
    widths: Vec<u8>,
    styles: Vec<u32>,
    cursor: String,
}

pub struct Renderer {
    depth: ColorDepth,
    previous: Option<Frame>,
}

impl Renderer {
    pub fn new(depth: ColorDepth) -> Renderer {
        Renderer { depth, previous: None }
    }

    /// Forget what the terminal shows, so the next frame redraws everything (after a resize, say).
    pub fn invalidate(&mut self) {
        self.previous = None;
    }

    /// Output that turns the last frame into `screen`, inside one synchronized update so the
    /// terminal never shows half a frame. The hardware cursor is shown at `cursor` (where
    /// input methods for Japanese and Chinese open), or hidden. Empty when nothing changed.
    pub fn frame(&mut self, screen: &Screen, cursor: Option<Cursor>) -> String {
        let previous = self.previous.as_ref();
        let full = previous.is_none_or(|previous| {
            previous.width != screen.width || previous.height != screen.height || !Rc::ptr_eq(&previous.table, &screen.table)
        });
        let cursor_key = cursor.map_or_else(String::new, |cursor| format!("{},{}", cursor.x, cursor.y));
        let mut body = String::new();
        let mut x0 = -1;
        let mut y0 = -1;
        let mut style: Option<u32> = None;
        for y in 0..screen.height.max(0) {
            for x in 0..screen.width.max(0) {
                let index = (y * screen.width + x) as usize;
                let cells = screen.widths[index];
                if cells == 0 {
                    continue;
                }
                if let (false, Some(previous)) = (full, previous) {
                    if previous.chars[index] == screen.chars[index]
                        && previous.widths[index] == cells
                        && previous.styles[index] == screen.styles[index]
                    {
                        continue;
                    }
                }
                if x != x0 || y != y0 {
                    body.push_str(&cursor_to(x, y));
                }
                let id = screen.styles[index];
                if style != Some(id) {
                    body.push_str(&sgr(&screen.table.style(id), self.depth));
                    style = Some(id);
                }
                body.push_str(&screen.chars[index]);
                x0 = x + cells as i32;
                y0 = y;
            }
        }
        let same_cursor = previous.is_some_and(|previous| previous.cursor == cursor_key);
        self.previous = Some(Frame {
            width: screen.width,
            height: screen.height,
            table: Rc::clone(&screen.table),
            cursor: cursor_key,
            chars: screen.chars.clone(),
            widths: screen.widths.clone(),
            styles: screen.styles.clone(),
        });
        if !full && body.is_empty() && same_cursor {
            return String::new();
        }
        let mut out = String::with_capacity(body.len() + 64);
        out.push_str(SYNC_START);
        out.push_str(HIDE_CURSOR);
        if full {
            out.push_str(RESET);
            out.push_str("\u{1b}[2J");
        }
        out.push_str(&body);
        out.push_str(RESET);
        if let Some(cursor) = cursor {
            out.push_str(&cursor_to(cursor.x, cursor.y));
            out.push_str(SHOW_CURSOR);
        }
        out.push_str(SYNC_END);
        out
    }
}
