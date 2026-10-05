//! Markdown as terminal rows: the subset models actually write in chat. Headings, bold,
//! italic, inline code, fenced code, lists with hanging indents, quotes, links and rules.
//! Single newlines stay line breaks, as in chat. Names like Kick_01_final stay literal.

use std::rc::Rc;
use std::sync::LazyLock;

use kumi_common::js;

use super::style::{palette, Rgb, Style};
use super::width::{graphemes, text_width};
use super::wrap::{wrap, Span};

#[derive(Clone, Debug)]
pub struct MarkdownRow {
    pub spans: Vec<Span>,
    /// Background band for code blocks.
    pub bg: Option<Rgb>,
}

impl MarkdownRow {
    fn plain(spans: Vec<Span>) -> MarkdownRow {
        MarkdownRow { spans, bg: None }
    }
}

// JavaScript's `.`: anything but a line terminator.
const DOT: &str = r"[^\n\r\u{2028}\u{2029}]";

static INLINE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(&format!(
        r"(`+)({DOT}+?)\1|\*\*(?=\S)({DOT}+?)(?<=\S)\*\*|(?<![A-Za-z0-9_*])\*(?=[^\s*])({DOT}+?)(?<=[^\s*])\*(?![A-Za-z0-9_*])|(?<![A-Za-z0-9_])_(?=\S)({DOT}+?)(?<=\S)_(?![A-Za-z0-9_])|\[([^\]\n]+)\]\((\S+?)\)"
    ))
    .unwrap()
});

/// Inline spans: `code`, **bold**, *italic*, _italic_ and [links](url).
pub fn inline(text: &str, base: &Rc<Style>) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut last = 0;
    // A search that hits the backtrack limit errs without moving on, so `flatten` would ask again
    // forever (a long line froze Kumi). The rest of such a line stays plain text.
    for found in INLINE.captures_iter(text).map_while(Result::ok) {
        let whole = found.get(0).expect("the match");
        let index = whole.start();
        if index > last {
            spans.push(Span::new(&text[last..index], base));
        }
        let group = |number: usize| found.get(number).map(|group| group.as_str());
        if let Some(code) = group(2) {
            spans.push(Span::styled(code, Style { fg: Some(palette::BRIGHT), bg: Some(palette::RAISED), ..**base }));
        } else if let Some(bold) = group(3) {
            spans.push(Span::styled(bold, Style { fg: Some(palette::BRIGHT), bold: true, ..**base }));
        } else if let Some(italic) = group(4).or_else(|| group(5)) {
            spans.push(Span::styled(italic, Style { italic: true, ..**base }));
        } else if let Some(label) = group(6) {
            spans.push(Span::styled(label, Style { underline: true, ..**base }));
            if let Some(url) = group(7).filter(|url| !url.is_empty() && *url != label) {
                spans.push(Span::styled(format!(" ({url})"), Style::fg(palette::FAINT)));
            }
        }
        last = whole.end();
    }
    if last < text.len() {
        spans.push(Span::new(&text[last..], base));
    }
    spans
}

/// Wrap spans to `width`, putting `first` before the first line and `rest` before the others.
fn hanging(spans: &[Span], width: i32, first: Span, rest: &str) -> Vec<MarkdownRow> {
    let indent = text_width(&first.text);
    wrap(spans, (width - indent).max(1))
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let lead = if index == 0 { first.clone() } else { Span::new(rest, &first.style) };
            MarkdownRow::plain(std::iter::once(lead).chain(line).collect())
        })
        .collect()
}

/// Split a long code line between characters; code is never word-wrapped.
fn code_rows(line: &str, width: i32, style: &Rc<Style>) -> Vec<MarkdownRow> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for grapheme in graphemes(line) {
        let cells = text_width(grapheme);
        if used + cells > width && !current.is_empty() {
            rows.push(MarkdownRow { spans: vec![Span::new(std::mem::take(&mut current), style)], bg: Some(palette::RAISED) });
            used = 0;
        }
        current.push_str(grapheme);
        used += cells;
    }
    rows.push(MarkdownRow { spans: vec![Span::new(current, style)], bg: Some(palette::RAISED) });
    rows
}

static OPENING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\s*(`{3,}|~{3,})").unwrap());
static HEADING: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"^\s{{0,3}}(#{{1,6}})\s+({DOT}*?)\s*#*\s*$")).unwrap());
static RULE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| fancy_regex::Regex::new(r"^\s{0,3}([-*_])(\s*\1){2,}\s*$").unwrap());
static QUOTE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(&format!(r"^\s{{0,3}}>\s?({DOT}*)$")).unwrap());
static ITEM: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"^(\s*)([-*+•]|[0-9]{{1,3}}[.)])\s+({DOT}*)$")).unwrap());
static TABLE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(&format!(r"^\s*\|{DOT}*\|\s*$")).unwrap());
static TABLE_RULE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\s*\|?\s*:?-{2,}").unwrap());

pub fn render_markdown(text: &str, width: i32, base: &Rc<Style>) -> Vec<MarkdownRow> {
    let mut rows: Vec<MarkdownRow> = Vec::new();
    let limit = width.max(1);
    let code = Rc::new(Style::fg(palette::BRIGHT));
    let faint = Rc::new(Style::fg(palette::FAINT));
    let mut fence: Option<String> = None;
    for line in text.split('\n') {
        let opening = OPENING.captures(line).map(|found| found[1].to_string());
        if let Some(open) = &fence {
            if opening.as_ref().is_some_and(|marker| marker.starts_with(open.as_str())) {
                fence = None;
                continue;
            }
            rows.extend(code_rows(line, limit, &code));
            continue;
        }
        if let Some(marker) = opening {
            fence = Some(marker);
            continue;
        }
        if js::string::trim(line).is_empty() {
            rows.push(MarkdownRow::plain(Vec::new()));
            continue;
        }
        if let Some(heading) = HEADING.captures(line) {
            let style = Rc::new(Style { fg: Some(palette::BRIGHT), bold: true, ..**base });
            rows.extend(wrap(&inline(&heading[2], &style), limit).into_iter().map(MarkdownRow::plain));
            continue;
        }
        if RULE.is_match(line).unwrap_or(false) {
            rows.push(MarkdownRow::plain(vec![Span::new("─".repeat(limit.min(24) as usize), &faint)]));
            continue;
        }
        if let Some(quote) = QUOTE.captures(line) {
            let dim = Rc::new(Style::fg(palette::DIM));
            rows.extend(hanging(&inline(&quote[1], &dim), limit, Span::styled("│ ", Style::fg(palette::RULE)), "│ "));
            continue;
        }
        if let Some(item) = ITEM.captures(line) {
            let depth = (item[1].replace('\t', "  ").chars().count() / 2).min(4);
            let marker = if item[2].bytes().any(|byte| byte.is_ascii_digit()) { item[2].to_string() } else { "•".to_string() };
            let lead = format!("{}{marker} ", "  ".repeat(depth));
            let rest = " ".repeat(text_width(&lead) as usize);
            rows.extend(hanging(&inline(&item[3], base), limit, Span::new(lead, &faint), &rest));
            continue;
        }
        if TABLE.is_match(line) {
            if TABLE_RULE.is_match(line) {
                continue;
            }
            rows.extend(wrap(&[Span::new(js::string::trim(line), base)], limit).into_iter().map(MarkdownRow::plain));
            continue;
        }
        rows.extend(wrap(&inline(line, base), limit).into_iter().map(MarkdownRow::plain));
    }
    rows
}
