//! Word wrapping of styled text into terminal lines.

use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;

use super::style::Style;
use super::width::{cell_width, graphemes, text_width};

/// Text in one style. Styles are shared by reference: adjacent text in the *same* style object
/// joins into one span when wrapped, as the TypeScript's identity check joined it.
#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: Rc<Style>,
}

impl Span {
    pub fn new(text: impl Into<String>, style: &Rc<Style>) -> Span {
        Span { text: text.into(), style: Rc::clone(style) }
    }

    /// A span in a style of its own (a fresh style object).
    pub fn styled(text: impl Into<String>, style: Style) -> Span {
        Span { text: text.into(), style: Rc::new(style) }
    }
}

static TOKENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n| +|[^ \n]+").unwrap());

/// A row that carries on a line wrapping broke: the cells its repeated lead takes (a list's indent, a quote's bar), and
/// whether copying joins it to the row above with a space (a break between words) or with nothing (a word, or a line of
/// code, split between characters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carry {
    pub lead: i32,
    pub space: bool,
}

/// Greedy word wrap to `width` cells. Newlines always break; a word wider than the line is
/// split between characters; spaces at a wrap point are dropped, but indentation at the
/// start of a paragraph is kept. Always returns at least one (possibly empty) line.
pub fn wrap(spans: &[Span], width: i32) -> Vec<Vec<Span>> {
    wrap_carried(spans, width).into_iter().map(|(line, _)| line).collect()
}

/// `wrap`, with how each line carries on the one before: none for a line a newline (or the text) starts.
pub fn wrap_carried(spans: &[Span], width: i32) -> Vec<(Vec<Span>, Option<Carry>)> {
    let limit = width.max(1);
    let mut lines = Lines::default();
    for span in spans {
        for token in TOKENS.find_iter(&span.text).map(|found| found.as_str()) {
            if token == "\n" {
                lines.break_line(false, false);
                continue;
            }
            let cells = text_width(token);
            if token.starts_with(' ') {
                if lines.continuation && lines.used == 0 {
                    continue;
                }
                if lines.used + cells > limit {
                    lines.break_line(true, true);
                    continue;
                }
                lines.append(token, &span.style, cells);
                continue;
            }
            if lines.used + cells <= limit {
                lines.append(token, &span.style, cells);
                continue;
            }
            if cells <= limit {
                lines.break_line(true, false);
                lines.append(token, &span.style, cells);
                continue;
            }
            for grapheme in graphemes(token) {
                let size = cell_width(grapheme);
                if lines.used + size > limit {
                    lines.break_line(true, false);
                }
                lines.append(grapheme, &span.style, size);
            }
        }
    }
    lines.lines.push((lines.line, lines.carry));
    lines.lines
}

/// The lines wrapped so far, and the one being filled: its cells, whether a wrap began it, and how it carries on the
/// line before.
#[derive(Default)]
struct Lines {
    lines: Vec<(Vec<Span>, Option<Carry>)>,
    line: Vec<Span>,
    used: i32,
    continuation: bool,
    carry: Option<Carry>,
}

impl Lines {
    fn append(&mut self, text: &str, style: &Rc<Style>, cells: i32) {
        match self.line.last_mut() {
            Some(last) if Rc::ptr_eq(&last.style, style) => last.text.push_str(text),
            _ => self.line.push(Span::new(text, style)),
        }
        self.used += cells;
    }

    /// Ends the line. A wrap (not a newline) drops the spaces at the break, and the next line carries this one on: with
    /// a space where there were some (or `spaced`, a space token the break swallowed), else with nothing.
    fn break_line(&mut self, wrapped: bool, spaced: bool) {
        let mut space = spaced;
        if wrapped {
            if let Some(last) = self.line.last_mut() {
                let trimmed = last.text.trim_end_matches(' ').len();
                space |= trimmed < last.text.len();
                last.text.truncate(trimmed);
                if last.text.is_empty() {
                    self.line.pop();
                }
            }
        }
        self.lines.push((std::mem::take(&mut self.line), self.carry.take()));
        self.used = 0;
        self.continuation = wrapped;
        self.carry = wrapped.then_some(Carry { lead: 0, space });
    }
}
