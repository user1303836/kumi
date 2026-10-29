//! The conversation in detail (ctrl+o): the full export's parts, as the session gave them, laid out as rows; searched
//! over whole lines however they wrapped; with jumps between requests. Its overlay draws it and takes its keys, and `[`
//! prints it as plain text.

use std::rc::Rc;

use kumi_agent::core::export::Part;

use super::markdown::render_markdown;
use super::screen::{Rect, Screen};
use super::style::{palette, Rgb, Style};
use super::width::{graphemes, text_width};
use super::wrap::{wrap_carried, Carry, Span};

/// A step's input and result show this many lines each; all of it is in the editor's copy (`v`).
pub const STEP_LINES: usize = 12;
/// What `[` prints at most: the last of it, from a line's start (as quitting prints the conversation).
pub const PRINTED_MOST: usize = 200 * 1024;

/// A row: its spans, drawn `indent` cells in; the band behind a request's (its colour and width); and where its own text
/// sits in its line (the line, and the bytes there), past the `lead` cells of its spans a wrapped row repeats.
struct Row {
    spans: Vec<Span>,
    indent: i32,
    band: Option<(Rgb, i32)>,
    line: usize,
    from: usize,
    len: usize,
    lead: i32,
}

/// The view laid out at one width: its rows, each line's whole text, and the row each request starts at.
struct Laid {
    width: i32,
    rows: Vec<Row>,
    lines: Vec<String>,
    requests: Vec<usize>,
}

/// A match of the search: its line, and the bytes it covers there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub struct Details {
    parts: Vec<Part>,
    /// The turn still under way, from Kumi's own transcript (the saved conversation doesn't have it yet): its steps.
    under_way: Vec<String>,
    laid: Option<Laid>,
    /// The first row on screen.
    pub top: usize,
    pub query: String,
    pub matches: Vec<Match>,
    pub current: usize,
}

fn span(text: impl Into<String>, color: Rgb) -> Span {
    Span::new(text, &Rc::new(Style::fg(color)))
}

/// `text` past its first `cells` cells.
fn past_cells(text: &str, cells: i32) -> &str {
    let mut width = 0;
    let mut at = 0;
    for grapheme in graphemes(text) {
        if width >= cells {
            break;
        }
        width += text_width(grapheme);
        at += grapheme.len();
    }
    &text[at..]
}

/// Lays the parts out at one width, row by row, keeping each line's whole text.
struct Builder {
    width: i32,
    rows: Vec<Row>,
    lines: Vec<String>,
    requests: Vec<usize>,
}

impl Builder {
    /// A row of a line: a new line where it doesn't carry one on (`carry`), joined to it with a space or nothing where
    /// it does; its own text is what's past its lead.
    fn row(&mut self, spans: Vec<Span>, band: Option<(Rgb, i32)>, carry: Option<Carry>, indent: i32) {
        match carry {
            Some(carry) if !self.lines.is_empty() => {
                if carry.space {
                    self.lines.last_mut().expect("a line").push(' ');
                }
            }
            _ => self.lines.push(String::new()),
        }
        let lead = carry.map_or(0, |carry| carry.lead);
        let text: String = spans.iter().map(|span| span.text.as_str()).collect();
        let own = past_cells(&text, lead);
        let line = self.lines.len() - 1;
        let from = self.lines[line].len();
        self.lines[line].push_str(own);
        self.rows.push(Row { spans, indent, band, line, from, len: own.len(), lead });
    }

    /// One line of `spans`, wrapped to the width less `indent`.
    fn line(&mut self, spans: &[Span], band: Option<Rgb>, indent: i32) {
        let wrapped = wrap_carried(spans, (self.width - indent).max(1));
        let width = wrapped.iter().map(|(spans, _)| spans.iter().map(|span| text_width(&span.text)).sum::<i32>()).max().unwrap_or(0);
        for (index, (spans, carry)) in wrapped.into_iter().enumerate() {
            self.row(spans, band.map(|bg| (bg, width + 2)), carry.filter(|_| index > 0), indent);
        }
    }

    fn gap(&mut self) {
        if !self.rows.is_empty() {
            self.row(vec![], None, None, 0);
        }
    }

    /// A step's input or result: its first [`STEP_LINES`] rows as they wrap (a long line of JSON counts by its rows), then
    /// how many lines there are past them, the one cut short among them.
    fn lines_of(&mut self, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        let mut shown = 0;
        for (index, line) in lines.iter().enumerate() {
            for (at, (spans, carry)) in wrap_carried(&[span(*line, palette::FAINT)], (self.width - 4).max(1)).into_iter().enumerate() {
                if shown == STEP_LINES {
                    let more = lines.len() - index;
                    let word = if more == 1 { "line" } else { "lines" };
                    self.line(&[span(format!("… {more} more {word}"), palette::FAINT)], None, 4);
                    return;
                }
                self.row(spans, None, carry.filter(|_| at > 0), 4);
                shown += 1;
            }
        }
    }

    fn part(&mut self, part: &Part, changes: &mut bool) {
        match part {
            Part::Fact { label, value } => self.line(&[span(format!("{label}: "), palette::FAINT), span(value, palette::DIM)], None, 0),
            Part::Earlier(asks) => {
                self.gap();
                self.line(&[span("What you asked before this part:", palette::FAINT)], None, 0);
                for ask in asks {
                    self.line(&[span(format!("• {ask}"), palette::FAINT)], None, 2);
                }
            }
            Part::Request(text) => {
                self.gap();
                self.requests.push(self.rows.len());
                for line in text.lines() {
                    self.line(&[span(line, palette::BRIGHT)], Some(palette::RAISED), 0);
                }
            }
            Part::Step { label, input, result } => {
                self.line(&[span("▸ ", palette::FAINT), span(label, palette::DIM)], None, 0);
                for text in [input, result].into_iter().flatten() {
                    self.lines_of(text);
                }
            }
            Part::Answer(text) => {
                self.gap();
                for row in render_markdown(text, self.width, &Rc::new(Style::fg(palette::TEXT))) {
                    self.row(row.spans.to_vec(), None, row.carry, 0);
                }
            }
            Part::Change { line, values } => {
                if !*changes {
                    *changes = true;
                    self.gap();
                    self.line(&[span("What Kumi changed", palette::BRIGHT)], None, 0);
                }
                self.line(&[span(format!("• {line}"), palette::TEXT)], None, 0);
                for value in values {
                    self.line(&[span(value, palette::DIM)], None, 4);
                }
            }
        }
    }
}

impl Details {
    pub fn new(parts: Vec<Part>, under_way: Vec<String>) -> Details {
        Details { parts, under_way, laid: None, top: 0, query: String::new(), matches: vec![], current: 0 }
    }

    fn lay_out(&mut self, width: i32) -> &Laid {
        if self.laid.as_ref().is_none_or(|laid| laid.width != width) {
            let mut builder = Builder { width, rows: vec![], lines: vec![], requests: vec![] };
            let mut changes = false;
            for part in &self.parts {
                builder.part(part, &mut changes);
            }
            if self.parts.is_empty() && self.under_way.is_empty() {
                builder.line(&[span("There's nothing to show yet: ask Kumi something first.", palette::FAINT)], None, 0);
            }
            if !self.under_way.is_empty() {
                builder.gap();
                builder.line(&[span("Under way", palette::BRIGHT)], None, 0);
                for step in &self.under_way {
                    builder.line(&[span("▸ ", palette::FAINT), span(step, palette::DIM)], None, 0);
                }
            }
            let Builder { rows, lines, requests, .. } = builder;
            self.laid = Some(Laid { width, rows, lines, requests });
        }
        self.laid.as_ref().expect("laid out")
    }

    /// How many rows the view has at `width`.
    pub fn rows(&mut self, width: i32) -> usize {
        self.lay_out(width).rows.len()
    }

    /// Searches every line for `query` (letters in any case), and goes to the first match at or below the top.
    pub fn search(&mut self, query: &str, width: i32) {
        self.query = query.to_string();
        self.matches.clear();
        let needle = query.to_ascii_lowercase();
        let top = self.top;
        let laid = self.lay_out(width);
        if needle.is_empty() {
            return;
        }
        let mut matches = vec![];
        for (line, text) in laid.lines.iter().enumerate() {
            let hay = text.to_ascii_lowercase();
            let mut at = 0;
            while let Some(found) = hay[at..].find(&needle) {
                let start = at + found;
                matches.push(Match { line, start, end: start + needle.len() });
                at = start + needle.len();
            }
        }
        let first_at_top = laid.rows.get(top).map_or(0, |row| row.line);
        self.current = matches.iter().position(|found| found.line >= first_at_top).unwrap_or(0);
        self.matches = matches;
    }

    /// The row a match starts on.
    fn row_of(&mut self, found: Match, width: i32) -> usize {
        let laid = self.lay_out(width);
        laid.rows.iter().position(|row| row.line == found.line && found.start < row.from + row.len.max(1)).unwrap_or(0)
    }

    /// The next match (or the one before), brought into view in an area `height` rows tall.
    pub fn next_match(&mut self, forward: bool, width: i32, height: usize) {
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len();
        self.current = if forward { (self.current + 1) % count } else { (self.current + count - 1) % count };
        self.show_match(width, height);
    }

    /// The current match in view: the top moves only if it isn't on screen.
    pub fn show_match(&mut self, width: i32, height: usize) {
        let Some(found) = self.matches.get(self.current).copied() else { return };
        let row = self.row_of(found, width);
        if row < self.top || row >= self.top + height {
            self.top = row.saturating_sub(height / 3);
        }
    }

    /// The next request below the top (or the one above it).
    pub fn next_request(&mut self, forward: bool, width: i32) {
        let top = self.top;
        let requests = &self.lay_out(width).requests;
        let to = if forward { requests.iter().find(|row| **row > top) } else { requests.iter().rev().find(|row| **row < top) };
        if let Some(row) = to {
            self.top = *row;
        }
    }

    /// Scrolls by `rows` (up where it's less than 0), within an area `height` rows tall.
    pub fn scroll(&mut self, rows: i32, width: i32, height: usize) {
        let most = self.rows(width).saturating_sub(height);
        self.top = (self.top as i64 + rows as i64).clamp(0, most as i64) as usize;
    }

    /// Draws the view into `area`, its text where the conversation's is; the search line at its foot while there's a
    /// query.
    pub fn draw(&mut self, screen: &mut Screen, area: Rect) {
        let (x, width) = (area.x + 3, (area.width - 5).max(1));
        screen.fill(area, &Style::bg(palette::GROUND));
        let searching = !self.query.is_empty();
        let height = (area.height - i32::from(searching)).max(0) as usize;
        let most = self.rows(width).saturating_sub(height);
        self.top = self.top.min(most);
        let (top, current) = (self.top, self.current);
        let matches = self.matches.clone();
        let laid = self.lay_out(width);
        for (index, row) in laid.rows.iter().skip(top).take(height).enumerate() {
            let y = area.y + index as i32;
            if let Some((bg, band)) = row.band {
                screen.fill(Rect::new(x - 1, y, band.min(width + 2), 1), &Style::bg(bg));
            }
            let mut column = x + row.indent;
            for span in &row.spans {
                column = screen.put_in(column, y, &span.text, &span.style, area);
            }
            // The matches on this row, over its text: the one gone to in the accent.
            let own = &laid.lines[row.line][row.from..row.from + row.len];
            for (at, found) in matches.iter().enumerate().filter(|(_, found)| found.line == row.line) {
                let (start, end) = (found.start.max(row.from), found.end.min(row.from + row.len));
                if start >= end {
                    continue;
                }
                let before = text_width(&own[..start - row.from]);
                let style = if at == current {
                    Style { fg: Some(palette::GROUND), bg: Some(palette::ACCENT), ..Style::default() }
                } else {
                    Style { fg: Some(palette::BRIGHT), bg: Some(palette::PULSE), ..Style::default() }
                };
                screen.put_in(x + row.indent + row.lead + before, y, &own[start - row.from..end - row.from], &style, area);
            }
        }
        if searching {
            let y = area.y + area.height - 1;
            screen.fill(Rect::new(area.x, y, area.width, 1), &Style::bg(palette::SURFACE));
            let found =
                if self.matches.is_empty() { "no match".to_string() } else { format!("{} of {}", self.current + 1, self.matches.len()) };
            let column = screen.put_in(x, y, "search: ", &Style { bg: Some(palette::SURFACE), ..Style::fg(palette::DIM) }, area);
            let column = screen.put_in(column, y, &self.query, &Style { bg: Some(palette::SURFACE), ..Style::fg(palette::BRIGHT) }, area);
            screen.put_in(column, y, &format!(" · {found}"), &Style { bg: Some(palette::SURFACE), ..Style::fg(palette::FAINT) }, area);
        }
    }

    /// The view as plain text for the terminal's scrollback (`[`), laid out at `width` with its steps cut as on screen,
    /// sanitized: the last [`PRINTED_MOST`] bytes of it, from a line's start.
    pub fn printed(&mut self, width: i32) -> String {
        let laid = self.lay_out(width);
        let text: String = laid
            .rows
            .iter()
            .map(|row| {
                let line: String = row.spans.iter().map(|span| span.text.as_str()).collect();
                format!("{}{}\n", " ".repeat(row.indent.max(0) as usize), line.trim_end())
            })
            .collect();
        // Straight to the terminal, past the screen's own guard: what came from tools and pages goes through the stream's
        // sanitizer first.
        let text = crate::text::sanitize_text(&text, &[]);
        if text.len() <= PRINTED_MOST {
            return text;
        }
        let mut cut = text.len() - PRINTED_MOST;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        match text[cut..].find('\n') {
            Some(at) => text[cut + at + 1..].to_string(),
            None => text[cut..].to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(answer: &str) -> Details {
        let input = (1..=20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        Details::new(
            vec![
                Part::Fact { label: "Model".into(), value: "gpt-5".into() },
                Part::Request("make the bass wider".into()),
                Part::Step { label: "looked at Bass".into(), input: Some(input), result: Some("ok".into()) },
                Part::Answer(answer.into()),
                Part::Request("and the pad".into()),
                Part::Answer("Done.".into()),
            ],
            vec!["looking at Pad".into()],
        )
    }

    fn texts(details: &mut Details, width: i32) -> Vec<String> {
        let laid = details.lay_out(width);
        laid.rows
            .iter()
            .map(|row| {
                let text: String = row.spans.iter().map(|span| span.text.as_str()).collect();
                format!("{}{}", " ".repeat(row.indent as usize), text.trim_end())
            })
            .collect()
    }

    #[test]
    fn steps_show_their_first_lines_and_requests_are_gone_to_in_turn() {
        let mut details = sample("Wider now.");
        let rows = texts(&mut details, 60);
        assert!(rows.contains(&"    line 12".to_string()) && !rows.iter().any(|row| row.trim() == "line 13"), "{rows:#?}");
        assert!(rows.contains(&"    … 8 more lines".to_string()), "{rows:#?}");
        // A result on one long line counts by the rows it wraps to.
        let long = format!("{{{}}}", (0..200).map(|n| format!("\"key{n}\":{n}")).collect::<Vec<_>>().join(","));
        let mut one_line = Details::new(vec![Part::Step { label: "read it".into(), input: None, result: Some(long) }], vec![]);
        let cut = texts(&mut one_line, 40);
        assert_eq!(cut.len(), 1 + STEP_LINES + 1, "{cut:#?}");
        assert_eq!(cut.last().unwrap(), "    … 1 more line");
        // Nothing to show says so.
        assert_eq!(texts(&mut Details::new(vec![], vec![]), 80), ["There's nothing to show yet: ask Kumi something first."]);
        assert!(rows.contains(&"Under way".to_string()) && rows.contains(&"▸ looking at Pad".to_string()), "{rows:#?}");
        for (forward, request) in [(true, "make the bass wider"), (true, "and the pad"), (false, "make the bass wider")] {
            details.next_request(forward, 60);
            assert_eq!(texts(&mut details, 60)[details.top], request);
        }
    }

    #[test]
    fn a_search_finds_words_a_wrap_split_and_marks_them_on_both_rows() {
        let mut details = sample("Use the low end of the kick.");
        // At 12 cells of text the answer wraps between "low" and "end".
        let rows = texts(&mut details, 12);
        let low = rows.iter().position(|row| row == "Use the low").expect("the wrap");
        assert_eq!(rows[low + 1], "end of the");
        details.search("LOW END", 12);
        assert_eq!(details.matches.len(), 1);
        let mut screen = Screen::new(17, 40);
        details.draw(&mut screen, Rect::new(0, 0, 17, 40));
        let lit = |x: i32, y: usize| screen.at(x, y as i32).style.bg == Some(palette::ACCENT);
        let first = 3 + text_width("Use the ");
        assert!((first..first + 3).all(|x| lit(x, low)) && !lit(first - 1, low), "low, lit");
        assert!((3..6).all(|x| lit(x, low + 1)) && !lit(6, low + 1), "end, lit");
        assert!(screen.lines()[39].trim_start().starts_with("search: LOW"), "the search line, cut to the narrow area");
    }

    #[test]
    fn what_goes_to_the_scrollback_is_sanitized_and_kept_to_its_last_part() {
        let mut details = sample("plain \u{1b}[31mred\u{1b}[0m and \u{7}a bell");
        let printed = details.printed(80);
        assert!(!printed.contains('\u{1b}') && !printed.contains('\u{7}') && printed.contains("red"), "{printed:?}");
        assert!(printed.contains("    … 8 more lines"));
        let long = (0..20_000).map(|n| format!("answer line {n}")).collect::<Vec<_>>().join("\n");
        let mut details = sample(&long);
        let printed = details.printed(80);
        assert!(printed.len() <= PRINTED_MOST && printed.starts_with("answer line "), "{}", &printed[..40]);
        assert!(printed.ends_with("▸ looking at Pad\n"));
    }
}
