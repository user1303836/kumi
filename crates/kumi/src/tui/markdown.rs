//! Markdown as terminal rows: the subset models actually write in chat. Headings, bold,
//! italic, inline code, fenced code, lists with hanging indents, quotes, links and rules.
//! Single newlines stay line breaks, as in chat. Names like Kick_01_final stay literal.

use std::rc::Rc;
use std::sync::LazyLock;

use kumi_common::js;

use super::style::{palette, Rgb, Style};
use super::width::{graphemes, text_width};
use super::wrap::{wrap_carried, Carry, Span};

#[derive(Clone, Debug)]
pub struct MarkdownRow {
    pub spans: Vec<Span>,
    /// Background band for code blocks.
    pub bg: Option<Rgb>,
    /// How the row carries on the one above, where wrapping broke a line.
    pub carry: Option<Carry>,
}

impl MarkdownRow {
    fn plain(spans: Vec<Span>) -> MarkdownRow {
        MarkdownRow { spans, bg: None, carry: None }
    }

    /// A line of text, wrapped to `width`.
    fn wrapped(spans: &[Span], width: i32) -> impl Iterator<Item = MarkdownRow> {
        wrap_carried(spans, width).into_iter().map(|(spans, carry)| MarkdownRow { spans, bg: None, carry })
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
    wrap_carried(spans, (width - indent).max(1))
        .into_iter()
        .enumerate()
        .map(|(index, (line, carry))| {
            let lead = if index == 0 { first.clone() } else { Span::new(rest, &first.style) };
            let carry = carry.map(|carry| Carry { lead: text_width(&lead.text), ..carry });
            MarkdownRow { spans: std::iter::once(lead).chain(line).collect(), bg: None, carry }
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
            rows.push(code_row(std::mem::take(&mut current), style, !rows.is_empty()));
            used = 0;
        }
        current.push_str(grapheme);
        used += cells;
    }
    rows.push(code_row(current, style, !rows.is_empty()));
    rows
}

/// A row of code; one that carries on the line before it joins it with nothing, its spaces being the code's own.
fn code_row(text: String, style: &Rc<Style>, carries: bool) -> MarkdownRow {
    MarkdownRow {
        spans: vec![Span::new(text, style)],
        bg: Some(palette::RAISED),
        carry: carries.then_some(Carry { lead: 0, space: false }),
    }
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
    let mut lines = Lines::new(width, base, None, Vec::new());
    for line in text.split('\n') {
        lines.line(line);
    }
    lines.rows
}

/// The rows of a text that grows line by line (an answer as it streams), rendered once per line: the rows of the
/// lines before the text's last newline are kept, with the fence state after them, and only what follows is rendered
/// again. A text that changed before that point, or another width, starts over.
#[derive(Default)]
pub struct MarkdownCache {
    width: i32,
    /// How much of the text the kept rows cover: up to and including its last newline.
    settled: usize,
    /// A hash of that much of the text, so a changed text is noticed.
    hash: u64,
    fence: Option<String>,
    rows: Vec<MarkdownRow>,
}

impl MarkdownCache {
    /// How many bytes of the last text the kept rows cover.
    pub fn settled(&self) -> usize {
        self.settled
    }
}

fn hash_of(text: &str) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write(text.as_bytes());
    hasher.finish()
}

/// `render_markdown`, keeping the lines rendered so far in `cache` for the next, longer text.
pub fn render_markdown_with(text: &str, width: i32, base: &Rc<Style>, cache: &mut MarkdownCache) -> Vec<MarkdownRow> {
    let settled = text.rfind('\n').map_or(0, |at| at + 1);
    // A changed text may end a character where the kept text ended a line: `get` says so instead of slicing.
    let kept = cache.width == width
        && cache.settled > 0
        && cache.settled <= settled
        && text.get(..cache.settled).is_some_and(|before| hash_of(before) == cache.hash);
    if !kept {
        *cache = MarkdownCache { width, ..MarkdownCache::default() };
    }
    if cache.settled < settled {
        let mut lines = Lines::new(width, base, cache.fence.take(), std::mem::take(&mut cache.rows));
        for line in text[cache.settled..settled - 1].split('\n') {
            lines.line(line);
        }
        cache.rows = lines.rows;
        cache.fence = lines.fence;
        cache.settled = settled;
        cache.hash = hash_of(&text[..settled]);
    }
    let mut lines = Lines::new(width, base, cache.fence.clone(), cache.rows.clone());
    lines.line(&text[settled..]);
    lines.rows
}

/// A text's lines becoming rows, one at a time: what carries from one line to the next is an open fence.
struct Lines<'a> {
    limit: i32,
    base: &'a Rc<Style>,
    code: Rc<Style>,
    faint: Rc<Style>,
    fence: Option<String>,
    rows: Vec<MarkdownRow>,
}

impl<'a> Lines<'a> {
    fn new(width: i32, base: &'a Rc<Style>, fence: Option<String>, rows: Vec<MarkdownRow>) -> Self {
        Lines {
            limit: width.max(1),
            base,
            code: Rc::new(Style::fg(palette::BRIGHT)),
            faint: Rc::new(Style::fg(palette::FAINT)),
            fence,
            rows,
        }
    }

    fn line(&mut self, line: &str) {
        let (limit, base) = (self.limit, self.base);
        let opening = OPENING.captures(line).map(|found| found[1].to_string());
        if let Some(open) = &self.fence {
            if opening.as_ref().is_some_and(|marker| marker.starts_with(open.as_str())) {
                self.fence = None;
                return;
            }
            self.rows.extend(code_rows(line, limit, &self.code));
            return;
        }
        if let Some(marker) = opening {
            self.fence = Some(marker);
            return;
        }
        if js::string::trim(line).is_empty() {
            self.rows.push(MarkdownRow::plain(Vec::new()));
            return;
        }
        if let Some(heading) = HEADING.captures(line) {
            let style = Rc::new(Style { fg: Some(palette::BRIGHT), bold: true, ..**base });
            self.rows.extend(MarkdownRow::wrapped(&inline(&heading[2], &style), limit));
            return;
        }
        if RULE.is_match(line).unwrap_or(false) {
            self.rows.push(MarkdownRow::plain(vec![Span::new("─".repeat(limit.min(24) as usize), &self.faint)]));
            return;
        }
        if let Some(quote) = QUOTE.captures(line) {
            let dim = Rc::new(Style::fg(palette::DIM));
            self.rows.extend(hanging(&inline(&quote[1], &dim), limit, Span::styled("│ ", Style::fg(palette::RULE)), "│ "));
            return;
        }
        if let Some(item) = ITEM.captures(line) {
            let depth = (item[1].replace('\t', "  ").chars().count() / 2).min(4);
            let marker = if item[2].bytes().any(|byte| byte.is_ascii_digit()) { item[2].to_string() } else { "•".to_string() };
            let lead = format!("{}{marker} ", "  ".repeat(depth));
            let rest = " ".repeat(text_width(&lead) as usize);
            self.rows.extend(hanging(&inline(&item[3], base), limit, Span::new(lead, &self.faint), &rest));
            return;
        }
        if TABLE.is_match(line) {
            if TABLE_RULE.is_match(line) {
                return;
            }
            self.rows.extend(MarkdownRow::wrapped(&[Span::new(js::string::trim(line), base)], limit));
            return;
        }
        self.rows.extend(MarkdownRow::wrapped(&inline(line, base), limit));
    }
}
