//! Turns screens into terminal output, writing only what changed since the last frame. Rows that moved up together
//! are moved by the terminal (a scrolling region), and long runs of blanks are erased rather than written.

use std::fmt::Write as _;
use std::ops::Range;
use std::rc::Rc;

use super::screen::{Screen, BLANK};
use super::style::{link_url, sgr, ColorDepth, Style, StyleTable};

pub fn cursor_to(x: i32, y: i32) -> String {
    format!("\u{1b}[{};{}H", y + 1, x + 1)
}

const SYNC_START: &str = "\u{1b}[?2026h";
const SYNC_END: &str = "\u{1b}[?2026l";
const HIDE_CURSOR: &str = "\u{1b}[?25l";
const SHOW_CURSOR: &str = "\u{1b}[?25h";
const RESET: &str = "\u{1b}[0m";
/// Changed blanks in one run before they're erased at once (EL, ECH) rather than written as spaces.
const ERASE_MIN: usize = 8;
/// Rows a move by the terminal has to save, past the right ones it blanks, before it's worth sending.
const SCROLL_MIN: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub x: i32,
    pub y: i32,
}

/// The last frame, as the terminal shows it.
struct Frame {
    width: i32,
    height: i32,
    table: Rc<StyleTable>,
    codes: Vec<u32>,
    widths: Vec<u8>,
    styles: Vec<u32>,
    /// Each row's hash: rows that moved are found by these.
    hashes: Vec<u64>,
    cursor: String,
}

impl Frame {
    /// Rows `top` to `bottom` moved up `by`, as the terminal moves them: the rows they leave are blank, in the
    /// terminal's own colours.
    fn scroll(&mut self, top: usize, bottom: usize, by: usize) {
        let width = self.width as usize;
        let (from, to, left) = (top * width, bottom * width, (bottom - by) * width);
        self.codes.copy_within(from + by * width..to, from);
        self.widths.copy_within(from + by * width..to, from);
        self.styles.copy_within(from + by * width..to, from);
        self.codes[left..to].fill(BLANK);
        self.widths[left..to].fill(1);
        self.styles[left..to].fill(0);
        self.hashes.copy_within(top + by..bottom, top);
        for y in bottom - by..bottom {
            let row = y * width..(y + 1) * width;
            self.hashes[y] = row_hash(&self.codes[row.clone()], &self.widths[row.clone()], &self.styles[row]);
        }
    }

    fn same_row(&self, screen: &Screen, row: Range<usize>) -> bool {
        self.codes[row.clone()] == screen.codes[row.clone()]
            && self.widths[row.clone()] == screen.widths[row.clone()]
            && self.styles[row.clone()] == screen.styles[row]
    }

    fn same_cell(&self, screen: &Screen, index: usize) -> bool {
        self.codes[index] == screen.codes[index] && self.widths[index] == screen.widths[index] && self.styles[index] == screen.styles[index]
    }
}

pub struct Renderer {
    depth: ColorDepth,
    previous: Option<Frame>,
    full_repaint: bool,
    erase: bool,
    links: bool,
}

impl Renderer {
    pub fn new(depth: ColorDepth) -> Renderer {
        Renderer { depth, previous: None, full_repaint: false, erase: true, links: true }
    }

    /// Every cell written in every frame that changes (the `fullRepaint` setting, for a terminal that draws changes
    /// wrongly): nothing is left as it was, erased or moved.
    pub fn full_repaint(self, on: bool) -> Renderer {
        Renderer { full_repaint: on, ..self }
    }

    /// Whether blanks may be erased (EL, ECH): not where the terminal erases in its own colours instead of the ones set.
    pub fn erasing(self, on: bool) -> Renderer {
        Renderer { erase: on, ..self }
    }

    /// Whether linked text opens its address (OSC 8): not where an OSC could show as text.
    pub fn linking(self, on: bool) -> Renderer {
        Renderer { links: on, ..self }
    }

    /// Forget what the terminal shows, so the next frame redraws everything (after a resize, say).
    pub fn invalidate(&mut self) {
        self.previous = None;
    }

    /// Output that turns the last frame into `screen`, inside one synchronized update so the
    /// terminal never shows half a frame. The hardware cursor is shown at `cursor` (where
    /// input methods for Japanese and Chinese open), or hidden. Empty when nothing changed.
    pub fn frame(&mut self, screen: &Screen, cursor: Option<Cursor>) -> String {
        let fresh = self.previous.as_ref().is_none_or(|previous| {
            previous.width != screen.width || previous.height != screen.height || !Rc::ptr_eq(&previous.table, &screen.table)
        });
        let cursor_key = cursor.map_or_else(String::new, |cursor| format!("{},{}", cursor.x, cursor.y));
        let (width, height) = (screen.width.max(0) as usize, screen.height.max(0) as usize);
        let hashes: Vec<u64> = (0..height)
            .map(|y| {
                let row = y * width..(y + 1) * width;
                row_hash(&screen.codes[row.clone()], &screen.widths[row.clone()], &screen.styles[row])
            })
            .collect();
        let mut pen =
            Pen { out: String::new(), table: &screen.table, depth: self.depth, links: self.links, at: None, style: None, link: 0 };
        let kept = self.previous.as_mut().filter(|_| !fresh);
        let mut same = vec![false; height];
        if let Some(previous) = kept {
            if !self.full_repaint {
                if let Some((top, bottom, by)) = shift(&previous.hashes, &hashes) {
                    pen.scroll(top, bottom, by);
                    previous.scroll(top, bottom, by);
                }
            }
            for (y, same) in same.iter_mut().enumerate() {
                *same = previous.hashes[y] == hashes[y] && previous.same_row(screen, y * width..(y + 1) * width);
            }
            if pen.out.is_empty() && same.iter().all(|same| *same) && previous.cursor == cursor_key {
                return String::new();
            }
        }
        // What the terminal shows to write over: nothing after a resize, or where every cell is written.
        let shown = self.previous.as_ref().filter(|_| !fresh && !self.full_repaint);
        let erase = self.erase && !self.full_repaint;
        for y in 0..height {
            if shown.is_some() && same[y] {
                continue;
            }
            pen.row(screen, shown, y, erase);
        }
        if pen.link != 0 {
            hyperlink(&mut pen.out, 0);
        }
        let body = pen.out;
        // The last frame's cells take this one's in place, not as new copies.
        match self.previous.as_mut().filter(|_| !fresh) {
            Some(previous) => {
                previous.codes.clone_from(&screen.codes);
                previous.widths.clone_from(&screen.widths);
                previous.styles.clone_from(&screen.styles);
                previous.hashes = hashes;
                previous.cursor = cursor_key;
            }
            None => {
                self.previous = Some(Frame {
                    width: screen.width,
                    height: screen.height,
                    table: Rc::clone(&screen.table),
                    codes: screen.codes.clone(),
                    widths: screen.widths.clone(),
                    styles: screen.styles.clone(),
                    hashes,
                    cursor: cursor_key,
                })
            }
        }
        let mut out = String::with_capacity(body.len() + 64);
        out.push_str(SYNC_START);
        out.push_str(HIDE_CURSOR);
        if fresh {
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

/// What a frame writes, and what it leaves set in the terminal as it goes: the cursor, the colours, the link open.
struct Pen<'a> {
    out: String,
    table: &'a StyleTable,
    depth: ColorDepth,
    links: bool,
    at: Option<(i32, i32)>,
    style: Option<u32>,
    link: u32,
}

impl Pen<'_> {
    fn to(&mut self, x: i32, y: i32) {
        if self.at != Some((x, y)) {
            let _ = write!(self.out, "\u{1b}[{};{}H", y + 1, x + 1);
            self.at = Some((x, y));
        }
    }

    fn style(&mut self, id: u32) {
        if self.style == Some(id) {
            return;
        }
        let style = self.table.style(id);
        self.out.push_str(&sgr(&style, self.depth));
        self.style = Some(id);
        let link = if self.links { style.link } else { 0 };
        if link != self.link {
            hyperlink(&mut self.out, link);
            self.link = link;
        }
    }

    /// Row `y` of `screen` over what `shown` has there: the cells that differ, or all of them without it. A run of
    /// blanks that differ in one style is erased at once, where `erase` allows.
    fn row(&mut self, screen: &Screen, shown: Option<&Frame>, y: usize, erase: bool) {
        let width = screen.width.max(0) as usize;
        let start = y * width;
        let differs = |x: usize| shown.is_none_or(|shown| !shown.same_cell(screen, start + x));
        // Cells before this were looked at as a run of blanks too short to erase.
        let mut plain = 0;
        let mut x = 0;
        while x < width {
            let index = start + x;
            if screen.widths[index] == 0 || !differs(x) {
                x += 1;
                continue;
            }
            let style = screen.styles[index];
            if erase && x >= plain && screen.codes[index] == BLANK && self.erasable(style) {
                let blank =
                    |at: usize| screen.codes[start + at] == BLANK && screen.widths[start + at] == 1 && screen.styles[start + at] == style;
                let end = (x..width).find(|at| !blank(*at)).unwrap_or(width);
                if (x..end).filter(|at| differs(*at)).count() >= ERASE_MIN {
                    self.to(x as i32, y as i32);
                    self.style(style);
                    // Erased to the end of the row, or as many as the run (the cursor stays where it was).
                    if end == width {
                        self.out.push_str("\u{1b}[K");
                    } else {
                        let _ = write!(self.out, "\u{1b}[{}X", end - x);
                    }
                    x = end;
                    continue;
                }
                plain = end;
            }
            self.to(x as i32, y as i32);
            self.style(style);
            screen.push_grapheme(index, &mut self.out);
            self.at = Some((x as i32 + i32::from(screen.widths[index]), y as i32));
            x += 1;
        }
    }

    /// Whether a blank in style `id` is what an erase leaves: a background and nothing else (no colour of text, no
    /// underline, inverse or link).
    fn erasable(&self, id: u32) -> bool {
        let style = self.table.style(id);
        style == Style { bg: style.bg, ..Style::default() }
    }

    /// Rows `top` to `bottom` moved up `by` in the terminal: a scrolling region, `by` lines deleted at its top (DL), and
    /// the region taken away. DL rather than SU: every terminal Kumi runs in deletes lines, where some ignore SU.
    fn scroll(&mut self, top: usize, bottom: usize, by: usize) {
        // The rows coming in at the region's foot are blank in the colours set: the terminal's own, then.
        self.out.push_str(RESET);
        self.style = None;
        let _ = write!(self.out, "\u{1b}[{};{bottom}r\u{1b}[{};1H\u{1b}[{by}M\u{1b}[r", top + 1, top + 1);
        // Taking the region away puts the cursor home.
        self.at = Some((0, 0));
    }
}

/// Opens link `id`'s address (OSC 8), or closes the one open with 0. The address goes out percent-encoded past
/// printable ASCII, all the sequence carries.
fn hyperlink(out: &mut String, id: u32) {
    out.push_str("\u{1b}]8;");
    match link_url(id) {
        Some(url) => {
            let _ = write!(out, "id=k{id};");
            for byte in url.bytes() {
                if (0x21..0x7f).contains(&byte) {
                    out.push(byte as char);
                } else {
                    let _ = write!(out, "%{byte:02X}");
                }
            }
        }
        None => out.push(';'),
    }
    out.push_str("\u{1b}\\");
}

/// A row's hash, to find where rows moved; a row is still compared cell by cell before it's left as it was.
fn row_hash(codes: &[u32], widths: &[u8], styles: &[u32]) -> u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for ((code, width), style) in codes.iter().zip(widths).zip(styles) {
        hash = (hash ^ (u64::from(*code) | u64::from(*width) << 32)).wrapping_mul(PRIME);
        hash = (hash ^ u64::from(*style)).wrapping_mul(PRIME);
    }
    hash
}

/// Rows that moved up together since the last frame, as the conversation does while text streams in under it: the
/// band from `top` to `bottom` for the terminal to move up `by` rows, where that saves the most rows from being
/// written, counting the right rows it blanks at the band's foot against it. None where it saves fewer than
/// SCROLL_MIN.
fn shift(before: &[u64], after: &[u64]) -> Option<(usize, usize, usize)> {
    let rows = after.len().min(before.len());
    let mut best = None;
    let mut most = SCROLL_MIN - 1;
    for by in 1..rows {
        let mut y = 0;
        while y + by < rows {
            if after[y] != before[y + by] {
                y += 1;
                continue;
            }
            let top = y;
            let mut saved = 0;
            while y + by < rows && after[y] == before[y + by] {
                saved += usize::from(after[y] != before[y]);
                y += 1;
            }
            let lost = (y..y + by).filter(|row| after[*row] == before[*row]).count();
            if saved > lost + most {
                most = saved - lost;
                best = Some((top, y + by, by));
            }
        }
    }
    best
}
