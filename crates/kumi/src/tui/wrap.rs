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

/// Greedy word wrap to `width` cells. Newlines always break; a word wider than the line is
/// split between characters; spaces at a wrap point are dropped, but indentation at the
/// start of a paragraph is kept. Always returns at least one (possibly empty) line.
pub fn wrap(spans: &[Span], width: i32) -> Vec<Vec<Span>> {
    let limit = width.max(1);
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut line: Vec<Span> = Vec::new();
    let mut used = 0;
    let mut continuation = false;

    fn append(line: &mut Vec<Span>, used: &mut i32, text: &str, style: &Rc<Style>, cells: i32) {
        match line.last_mut() {
            Some(last) if Rc::ptr_eq(&last.style, style) => last.text.push_str(text),
            _ => line.push(Span::new(text, style)),
        }
        *used += cells;
    }
    fn break_line(lines: &mut Vec<Vec<Span>>, line: &mut Vec<Span>, used: &mut i32, continuation: &mut bool, wrapped: bool) {
        if wrapped {
            if let Some(last) = line.last_mut() {
                let trimmed = last.text.trim_end_matches(' ').len();
                last.text.truncate(trimmed);
                if last.text.is_empty() {
                    line.pop();
                }
            }
        }
        lines.push(std::mem::take(line));
        *used = 0;
        *continuation = wrapped;
    }

    for span in spans {
        for token in TOKENS.find_iter(&span.text).map(|found| found.as_str()) {
            if token == "\n" {
                break_line(&mut lines, &mut line, &mut used, &mut continuation, false);
                continue;
            }
            let cells = text_width(token);
            if token.starts_with(' ') {
                if continuation && used == 0 {
                    continue;
                }
                if used + cells > limit {
                    break_line(&mut lines, &mut line, &mut used, &mut continuation, true);
                    continue;
                }
                append(&mut line, &mut used, token, &span.style, cells);
                continue;
            }
            if used + cells <= limit {
                append(&mut line, &mut used, token, &span.style, cells);
                continue;
            }
            if cells <= limit {
                break_line(&mut lines, &mut line, &mut used, &mut continuation, true);
                append(&mut line, &mut used, token, &span.style, cells);
                continue;
            }
            for grapheme in graphemes(token) {
                let size = cell_width(grapheme);
                if used + size > limit {
                    break_line(&mut lines, &mut line, &mut used, &mut continuation, true);
                }
                append(&mut line, &mut used, grapheme, &span.style, size);
            }
        }
    }
    lines.push(line);
    lines
}
