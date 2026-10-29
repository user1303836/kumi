//! Markdown as terminal rows, from pulldown-cmark's events: headings, bold, italic, inline code, fenced code, lists
//! with hanging indents, quotes, links, rules, and tables laid out to the width (each row as lines of a heading and its
//! value where the columns won't fit). Single newlines stay line breaks, as in chat, and a blank line is a blank row.
//! Names like Kick_01_final stay literal, and so does a `*` inside a word (2*3*4). The parser doesn't backtrack: a long
//! line costs its length, never more.

use std::ops::Range;
use std::rc::Rc;

use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};

use super::style::{palette, Rgb, Style};
use super::width::{graphemes, text_width};
use super::wrap::{wrap, wrap_carried, Carry, Span};

#[derive(Clone, Debug)]
pub struct MarkdownRow {
    /// Shared, so the rows an answer keeps while it streams aren't copied for each frame.
    pub spans: Rc<[Span]>,
    /// Background band for code blocks.
    pub bg: Option<Rgb>,
    /// How the row carries on the one above, where wrapping broke a line.
    pub carry: Option<Carry>,
}

impl MarkdownRow {
    fn plain(spans: Vec<Span>) -> MarkdownRow {
        MarkdownRow { spans: spans.into(), bg: None, carry: None }
    }
}

/// The events of `text`. Tables only: strikethrough, task lists, footnotes and smart quotes stay as they were typed.
fn events(text: &str) -> impl Iterator<Item = (Event<'_>, Range<usize>)> {
    Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter()
}

/// An emphasis with a `*` inside a word (2*3*4, foo*bar*) is the word's own text, as chat writes it.
fn in_word(text: &str, range: &Range<usize>) -> bool {
    let alphanumeric = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    text[range.clone()].starts_with('*')
        && (alphanumeric(text[..range.start].chars().next_back()) || alphanumeric(text[range.end..].chars().next()))
}

/// Whether an event's text lies inside an emphasis drawn as its own text: its words and its end, not the block around it.
fn inside(range: &Range<usize>, skipped: &Range<usize>) -> bool {
    skipped.contains(&range.start) && range.end <= skipped.end
}

/// Inline spans: `code`, **bold**, *italic*, _italic_ and [links](url), the text of each block after the last.
pub fn inline(text: &str, base: &Rc<Style>) -> Vec<Span> {
    let faint = Rc::new(Style::fg(palette::FAINT));
    let mut gathered = Inline::new(base.clone());
    let mut skip = 0..0;
    for (event, range) in events(text) {
        if inside(&range, &skip) {
            continue;
        }
        match event {
            Event::Start(Tag::Emphasis) if in_word(text, &range) => {
                gathered.push(&text[range.clone()]);
                skip = range;
            }
            Event::SoftBreak | Event::HardBreak => gathered.push(" "),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => gathered.push(" "),
            event => gathered.event(event, &faint),
        }
    }
    let mut spans = gathered.lines.concat();
    if let Some(last) = spans.last_mut().filter(|last| last.text.ends_with(' ')) {
        let kept = last.text.trim_end_matches(' ').len();
        last.text.truncate(kept);
    }
    spans.retain(|span| !span.text.is_empty());
    spans
}

/// Inline text gathered as spans, line by line (a break, or a single newline, starts one), with the styles of the
/// elements open around it and the links whose words are still coming.
struct Inline {
    lines: Vec<Vec<Span>>,
    styles: Vec<Rc<Style>>,
    links: Vec<(String, String)>,
}

impl Inline {
    fn new(base: Rc<Style>) -> Inline {
        Inline { lines: vec![vec![]], styles: vec![base], links: vec![] }
    }

    fn style(&self) -> Rc<Style> {
        self.styles.last().cloned().expect("the base style")
    }

    fn push(&mut self, text: &str) {
        self.push_styled(text, self.style());
    }

    /// Text in `style`: joined to the span before it when that's in the same style, as wrapping joins them.
    fn push_styled(&mut self, text: &str, style: Rc<Style>) {
        if let Some((_, label)) = self.links.last_mut() {
            label.push_str(text);
        }
        let line = self.lines.last_mut().expect("a line");
        match line.last_mut() {
            Some(last) if Rc::ptr_eq(&last.style, &style) => last.text.push_str(text),
            _ => line.push(Span::new(text, &style)),
        }
    }

    fn open(&mut self, styled: impl FnOnce(Style) -> Style) {
        let style = Rc::new(styled(*self.style()));
        self.styles.push(style);
    }

    fn close(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn event(&mut self, event: Event<'_>, faint: &Rc<Style>) {
        match event {
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.push(&text),
            Event::InlineMath(text) | Event::DisplayMath(text) | Event::FootnoteReference(text) => self.push(&text),
            Event::Code(code) => {
                let style = Style { fg: Some(palette::BRIGHT), bg: Some(palette::RAISED), ..*self.style() };
                self.push_styled(&code, Rc::new(style));
            }
            Event::SoftBreak | Event::HardBreak => self.lines.push(vec![]),
            Event::TaskListMarker(done) => self.push(if done { "[x] " } else { "[ ] " }),
            Event::Start(Tag::Emphasis) => self.open(|style| Style { italic: true, ..style }),
            Event::Start(Tag::Strong) => self.open(|style| Style { fg: Some(palette::BRIGHT), bold: true, ..style }),
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                self.open(|style| Style { underline: true, ..style });
                self.links.push((dest_url.to_string(), String::new()));
            }
            Event::Start(Tag::Strikethrough | Tag::Superscript | Tag::Subscript) => self.open(|style| style),
            Event::End(TagEnd::Link | TagEnd::Image) => {
                self.close();
                // The address after the words, unless the words are the address.
                if let Some((url, _)) = self.links.pop().filter(|(url, label)| !url.is_empty() && url != label) {
                    self.push_styled(&format!(" ({url})"), faint.clone());
                }
            }
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Superscript | TagEnd::Subscript) => self.close(),
            _ => {}
        }
    }
}

pub fn render_markdown(text: &str, width: i32, base: &Rc<Style>) -> Vec<MarkdownRow> {
    Render::new(text, width, base, false).run().rows
}

/// The rows of a text that grows (an answer as it streams), its blocks rendered once they're done: the rows of the
/// blocks before the last one in its complete lines are kept, since nothing after a block changes it, and only the
/// rest is rendered again. A text that changed before that point, or another width, starts over. While `streaming`, a
/// table at the end is held back, since a new row can change every column.
#[derive(Default)]
pub struct MarkdownCache {
    width: i32,
    /// How much of the text the kept rows cover: the blocks before the last one that began in its complete lines.
    settled: usize,
    /// A hash of that much of the text, so a changed text is noticed.
    hash: u64,
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

/// `render_markdown`, keeping the blocks rendered so far in `cache` for the next, longer text.
pub fn render_markdown_with(text: &str, width: i32, base: &Rc<Style>, cache: &mut MarkdownCache, streaming: bool) -> Vec<MarkdownRow> {
    // A changed text may end a character where the kept text ended a line: `get` says so instead of slicing.
    let kept = cache.width == width && cache.settled > 0 && text.get(..cache.settled).is_some_and(|before| hash_of(before) == cache.hash);
    if !kept {
        *cache = MarkdownCache { width, ..MarkdownCache::default() };
    }
    let rest = &text[cache.settled..];
    let rendered = Render::new(rest, width, base, streaming).run();
    // The blocks before the last one that began in the complete lines are done: a later line can't change them.
    let complete = rest.rfind('\n').map_or(0, |at| at + 1);
    let mut from = 0;
    if let Some(last) = rendered.tops.iter().rev().find(|top| top.start < complete).filter(|last| last.chunk > 0) {
        cache.rows.extend(rendered.rows[..last.row].iter().cloned());
        cache.settled += last.chunk;
        cache.hash = hash_of(&text[..cache.settled]);
        from = last.row;
    }
    let mut rows = Vec::with_capacity(cache.rows.len() + rendered.rows.len() - from);
    rows.extend(cache.rows.iter().cloned());
    rows.extend(rendered.rows.into_iter().skip(from));
    rows
}

/// What's drawn around a block: a quote's bar, or a list item's marker (on its first row; spaces under it after).
enum Around {
    Quote,
    Item { marker: String, first: bool },
}

/// A block at the top level: where its chunk begins (the line after the block before, its blank lines first), where the
/// block itself begins, and its chunk's first row.
struct Top {
    chunk: usize,
    start: usize,
    row: usize,
}

struct Rendered {
    rows: Vec<MarkdownRow>,
    tops: Vec<Top>,
}

/// A table as it's read: its columns' alignments, its heads and its rows of cells.
struct Table {
    aligns: Vec<Alignment>,
    head: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
}

struct Render<'a> {
    text: &'a str,
    /// Where each line of the text starts.
    starts: Vec<usize>,
    width: i32,
    base: Rc<Style>,
    faint: Rc<Style>,
    rule: Rc<Style>,
    dim: Rc<Style>,
    code: Rc<Style>,
    streaming: bool,
    rows: Vec<MarkdownRow>,
    around: Vec<Around>,
    /// Each list open, with the number its next item takes (None for a list of bullets).
    lists: Vec<Option<u64>>,
    /// The line after the last one drawn: the lines from there to the next block are blank rows.
    next_line: usize,
    /// The paragraph, heading or table cell whose words are being gathered; `implicit` for a tight list item's words,
    /// which come with no paragraph around them, and end with the next block or the item; where its last words end.
    inline: Option<Inline>,
    implicit: bool,
    inline_end: usize,
    /// The code (or HTML) block being gathered.
    code_text: Option<String>,
    table: Option<Table>,
    /// The last table drawn, while nothing came after it: its first row, how many rows it took, its rows' leads and
    /// how many rows of cells it has.
    held: Option<(usize, usize, Vec<Span>, usize)>,
    tops: Vec<Top>,
    /// How many blocks are open around the next event.
    depth: usize,
    /// The emphasis drawn as its own text: the events inside it are skipped.
    skip: Range<usize>,
}

impl<'a> Render<'a> {
    fn new(text: &'a str, width: i32, base: &Rc<Style>, streaming: bool) -> Self {
        Render {
            text,
            starts: std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1)).collect(),
            width: width.max(1),
            base: base.clone(),
            faint: Rc::new(Style::fg(palette::FAINT)),
            rule: Rc::new(Style::fg(palette::RULE)),
            dim: Rc::new(Style::fg(palette::DIM)),
            code: Rc::new(Style::fg(palette::BRIGHT)),
            streaming,
            rows: vec![],
            around: vec![],
            lists: vec![],
            next_line: 0,
            inline: None,
            implicit: false,
            inline_end: 0,
            code_text: None,
            table: None,
            held: None,
            tops: vec![],
            depth: 0,
            skip: 0..0,
        }
    }

    fn run(mut self) -> Rendered {
        let text = self.text;
        for (event, range) in events(text) {
            if inside(&range, &self.skip) {
                continue;
            }
            match event {
                Event::Start(tag) => self.start(tag, range),
                Event::End(tag) => self.end(tag, range),
                Event::Rule => {
                    self.flush();
                    self.top(range.start);
                    self.leaf(range.start);
                    let (lead, _) = self.leads();
                    let rule = "─".repeat((self.width - self.lead_width()).clamp(1, 24) as usize);
                    self.rows.push(MarkdownRow::plain(lead.into_iter().chain([Span::new(rule, &self.faint)]).collect()));
                    self.after(range.end);
                }
                Event::Text(text) | Event::Html(text) if self.code_text.is_some() => {
                    if let Some(code) = &mut self.code_text {
                        code.push_str(&text);
                    }
                }
                event => {
                    let faint = self.faint.clone();
                    self.inline_at(range.start).event(event, &faint);
                    self.inline_end = range.end;
                }
            }
        }
        // Blank lines after the last block are blank rows too.
        self.gap(self.starts.len());
        if let Some((row, count, lead, cells)) = self.held.take().filter(|_| self.streaming) {
            let said = format!("table · {cells} {} so far", if cells == 1 { "row" } else { "rows" });
            let placeholder = MarkdownRow::plain(lead.into_iter().chain([Span::new(said, &self.faint)]).collect());
            self.rows.splice(row..row + count, [placeholder]);
        }
        Rendered { rows: self.rows, tops: self.tops }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        let block = !matches!(
            tag,
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Superscript | Tag::Subscript | Tag::Link { .. } | Tag::Image { .. }
        );
        if block {
            self.flush();
            self.top(range.start);
            self.depth += 1;
        }
        match tag {
            Tag::Paragraph => {
                self.leaf(range.start);
                self.inline = Some(Inline::new(self.text_style()));
            }
            Tag::Heading { .. } => {
                self.leaf(range.start);
                self.inline = Some(Inline::new(Rc::new(Style { fg: Some(palette::BRIGHT), bold: true, ..*self.text_style() })));
            }
            Tag::BlockQuote(_) => self.around.push(Around::Quote),
            Tag::List(first) => self.lists.push(first),
            Tag::Item => {
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.around.push(Around::Item { marker, first: true });
            }
            Tag::CodeBlock(_) | Tag::HtmlBlock => {
                self.leaf(range.start);
                self.code_text = Some(String::new());
            }
            Tag::Table(aligns) => {
                self.leaf(range.start);
                self.table = Some(Table { aligns, head: vec![], rows: vec![], row: vec![] });
            }
            Tag::TableCell => self.inline = Some(Inline::new(self.text_style())),
            // A table's rows are gathered at their ends; footnotes, definition lists and metadata aren't turned on.
            _ if block => {}
            Tag::Emphasis if in_word(self.text, &range) => {
                let text = self.text;
                self.inline_at(range.start).push(&text[range.clone()]);
                self.inline_end = range.end;
                self.skip = range;
            }
            tag => {
                let faint = self.faint.clone();
                self.inline_at(range.start).event(Event::Start(tag), &faint);
            }
        }
    }

    /// The words being gathered: a tight list item's, which come with no paragraph, from `at` on.
    fn inline_at(&mut self, at: usize) -> &mut Inline {
        if self.inline.is_none() {
            self.leaf(at);
            self.inline = Some(Inline::new(self.text_style()));
            self.implicit = true;
        }
        self.inline.as_mut().expect("words being gathered")
    }

    /// A tight list item's words, drawn as the next block (or the item's end) comes.
    fn flush(&mut self) {
        if std::mem::take(&mut self.implicit) {
            if let Some(inline) = self.inline.take() {
                self.text_rows(inline.lines);
                self.after(self.inline_end);
            }
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        let block = !matches!(
            tag,
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Superscript
                | TagEnd::Subscript
                | TagEnd::Link
                | TagEnd::Image
        );
        if block {
            self.flush();
            self.depth = self.depth.saturating_sub(1);
        }
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                if let Some(inline) = self.inline.take() {
                    self.text_rows(inline.lines);
                }
                self.after(range.end);
            }
            TagEnd::CodeBlock => {
                let code = self.code_text.take().unwrap_or_default();
                self.code_rows(&code);
                self.after(range.end);
            }
            TagEnd::HtmlBlock => {
                let html = self.code_text.take().unwrap_or_default();
                let lines = html.strip_suffix('\n').unwrap_or(&html).split('\n').map(|line| vec![Span::new(line, &self.base)]).collect();
                self.text_rows(lines);
                self.after(range.end);
            }
            TagEnd::BlockQuote(_) => {
                // Its own lines after its last block (a `>` on its own) are rows of it.
                self.gap(self.line_of(range.end.saturating_sub(1)) + 1);
                self.around.pop();
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                // An item with nothing in it is its marker.
                if matches!(self.around.last(), Some(Around::Item { first: true, .. })) {
                    self.leaf(range.start);
                    let (lead, _) = self.leads();
                    self.rows.push(MarkdownRow::plain(lead));
                    self.after(range.end);
                }
                self.around.pop();
            }
            TagEnd::TableCell => {
                let cell = self.inline.take().map(|inline| inline.lines.concat()).unwrap_or_default();
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = std::mem::take(&mut table.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    let first = self.rows.len();
                    let lead_width = self.lead_width();
                    let mut lead = vec![];
                    for (index, line) in
                        table_lines(&table, self.width - lead_width, &self.text_style(), &self.faint).into_iter().enumerate()
                    {
                        let (leads, _) = self.leads();
                        if index == 0 {
                            lead = leads.iter().map(|span| Span::new(" ".repeat(text_width(&span.text) as usize), &span.style)).collect();
                        }
                        self.rows.push(MarkdownRow::plain(leads.into_iter().chain(line).collect()));
                    }
                    self.held = Some((first, self.rows.len() - first, lead, table.rows.len()));
                }
                self.after(range.end);
            }
            _ if block => {}
            tag => {
                let faint = self.faint.clone();
                if let Some(inline) = &mut self.inline {
                    inline.event(Event::End(tag), &faint);
                }
                self.inline_end = range.end;
            }
        }
    }

    /// A block beginning at the top level: where its chunk and itself begin, and its first row.
    fn top(&mut self, start: usize) {
        if self.depth == 0 {
            let chunk = self.starts.get(self.next_line).copied().unwrap_or(self.text.len());
            self.tops.push(Top { chunk, start: self.starts[self.line_of(start)], row: self.rows.len() });
        }
    }

    /// A block beginning at `start`: the blank lines before it become rows, and a table drawn before is no longer last.
    fn leaf(&mut self, start: usize) {
        self.gap(self.line_of(start));
        self.held = None;
    }

    /// The block that ended at `end` took the lines up to its last.
    fn after(&mut self, end: usize) {
        self.next_line = self.next_line.max(self.line_of(end.saturating_sub(1)) + 1);
    }

    fn line_of(&self, at: usize) -> usize {
        self.starts.partition_point(|start| *start <= at).saturating_sub(1)
    }

    /// The lines from the last block drawn up to `line`: a blank one is a blank row, a quote's `>` on its own is its
    /// bar, and anything else no block took (a link's reference) is shown as it is.
    fn gap(&mut self, line: usize) {
        while self.next_line < line {
            let start = self.starts[self.next_line];
            let end = self.starts.get(self.next_line + 1).map_or(self.text.len(), |next| next - 1);
            let words = self.text[start..end].trim_end_matches('\r');
            let trimmed = words.trim();
            if trimmed.is_empty() {
                self.rows.push(MarkdownRow::plain(vec![]));
            } else if trimmed.chars().all(|c| c == '>' || c.is_whitespace()) {
                let bars = "│ ".repeat(trimmed.chars().filter(|c| *c == '>').count());
                self.rows.push(MarkdownRow::plain(vec![Span::new(bars, &self.rule)]));
            } else {
                for (spans, carry) in wrap_carried(&[Span::new(words, &self.base)], self.width) {
                    self.rows.push(MarkdownRow { spans: spans.into(), bg: None, carry });
                }
            }
            self.next_line += 1;
        }
    }

    /// Words inside a quote are dim.
    fn text_style(&self) -> Rc<Style> {
        if self.around.iter().any(|around| matches!(around, Around::Quote)) {
            self.dim.clone()
        } else {
            self.base.clone()
        }
    }

    fn lead_width(&self) -> i32 {
        self.around
            .iter()
            .map(|around| match around {
                Around::Quote => 2,
                Around::Item { marker, .. } => text_width(marker),
            })
            .sum()
    }

    /// What goes before a row, from the quotes and items around it, and how many cells it takes: an item's marker on
    /// its first row and spaces under it after.
    fn leads(&mut self) -> (Vec<Span>, i32) {
        let (mut spans, mut width) = (vec![], 0);
        for around in &mut self.around {
            match around {
                Around::Quote => {
                    spans.push(Span::new("│ ", &self.rule));
                    width += 2;
                }
                Around::Item { marker, first } => {
                    let cells = text_width(marker);
                    spans.push(if std::mem::take(first) {
                        Span::new(marker.clone(), &self.faint)
                    } else {
                        Span::new(" ".repeat(cells as usize), &self.faint)
                    });
                    width += cells;
                }
            }
        }
        (spans, width)
    }

    /// Lines of words, each wrapped to the room the leads leave; a row that carries on a wrapped line says so, with
    /// the cells its leads take.
    fn text_rows(&mut self, lines: Vec<Vec<Span>>) {
        let room = (self.width - self.lead_width()).max(1);
        for line in lines {
            for (spans, carry) in wrap_carried(&line, room) {
                let (mut row, lead) = self.leads();
                row.extend(spans);
                self.rows.push(MarkdownRow { spans: row.into(), bg: None, carry: carry.map(|carry| Carry { lead, ..carry }) });
            }
        }
    }

    /// Code on a band, each line split between characters where it's longer than the room: code is never
    /// word-wrapped, and its rows carry on with nothing between, the spaces being the code's own.
    fn code_rows(&mut self, code: &str) {
        if code.is_empty() {
            return;
        }
        let room = (self.width - self.lead_width()).max(1);
        for line in code.strip_suffix('\n').unwrap_or(code).split('\n') {
            let mut pieces = vec![String::new()];
            let mut used = 0;
            for grapheme in graphemes(line) {
                let cells = text_width(grapheme);
                if used + cells > room && !pieces.last().is_some_and(String::is_empty) {
                    pieces.push(String::new());
                    used = 0;
                }
                pieces.last_mut().expect("a piece").push_str(grapheme);
                used += cells;
            }
            for (index, piece) in pieces.into_iter().enumerate() {
                let (mut spans, lead) = self.leads();
                spans.push(Span::new(piece, &self.code));
                let carry = (index > 0).then_some(Carry { lead, space: false });
                self.rows.push(MarkdownRow { spans: spans.into(), bg: Some(palette::RAISED), carry });
            }
        }
    }
}

/// The fewest cells a column is squeezed to before a table is shown a row at a time instead.
const NARROWEST: i32 = 6;

/// A table as lines: its columns laid out to `width` with two spaces between, numbers to the right, the heads in bold
/// over a rule, and a cell too long for its column wrapped inside it. Where the columns won't fit even squeezed, each
/// row becomes lines of a head and its value, with a blank line between rows.
fn table_lines(table: &Table, width: i32, base: &Rc<Style>, faint: &Rc<Style>) -> Vec<Vec<Span>> {
    let columns = table.aligns.len().max(table.head.len()).max(1);
    let cell = |cells: &[Vec<Span>], index: usize| cells.get(index).cloned().unwrap_or_default();
    let words = |spans: &[Span]| spans.iter().map(|span| span.text.as_str()).collect::<String>();
    let mut widths: Vec<i32> = (0..columns)
        .map(|index| {
            std::iter::once(&table.head).chain(&table.rows).map(|row| text_width(&words(&cell(row, index)))).max().unwrap_or(0).max(1)
        })
        .collect();
    let narrowest: Vec<i32> = widths.iter().map(|width| (*width).min(NARROWEST)).collect();
    let total = |widths: &[i32]| widths.iter().sum::<i32>() + 2 * (columns as i32 - 1);
    while total(&widths) > width {
        let Some(widest) = (0..columns).filter(|index| widths[*index] > narrowest[*index]).max_by_key(|index| widths[*index]) else {
            return table_by_rows(table, width, faint);
        };
        widths[widest] -= 1;
    }
    let right: Vec<bool> = (0..columns)
        .map(|index| match table.aligns.get(index) {
            Some(Alignment::Right) => true,
            Some(Alignment::Left | Alignment::Center) => false,
            _ => {
                let mut cells = table.rows.iter().map(|row| words(&cell(row, index))).filter(|cell| !cell.trim().is_empty()).peekable();
                cells.peek().is_some() && cells.all(|cell| numeric(&cell))
            }
        })
        .collect();
    let centered: Vec<bool> = (0..columns).map(|index| table.aligns.get(index) == Some(&Alignment::Center)).collect();
    let bold = |spans: Vec<Span>| -> Vec<Span> {
        spans.into_iter().map(|span| Span::styled(span.text, Style { bold: true, ..*span.style })).collect()
    };
    let mut lines = vec![];
    let row_lines = |cells: Vec<Vec<Span>>, lines: &mut Vec<Vec<Span>>| {
        let wrapped: Vec<Vec<Vec<Span>>> = cells.iter().zip(&widths).map(|(cell, width)| wrap(cell, *width)).collect();
        for at in 0..wrapped.iter().map(Vec::len).max().unwrap_or(1) {
            let mut line = vec![];
            for (index, cell) in wrapped.iter().enumerate() {
                let piece = cell.get(at).cloned().unwrap_or_default();
                let room = widths[index] - text_width(&words(&piece));
                let (before, after) = if right[index] {
                    (room, 0)
                } else if centered[index] {
                    (room / 2, room - room / 2)
                } else {
                    (0, room)
                };
                if index > 0 {
                    line.push(Span::new("  ", base));
                }
                line.push(Span::new(" ".repeat(before.max(0) as usize), base));
                line.extend(piece);
                line.push(Span::new(" ".repeat(after.max(0) as usize), base));
            }
            // Nothing after the words but the padding: a copy of the row ends with its words.
            while line.last().is_some_and(|span| span.text.trim().is_empty()) {
                line.pop();
            }
            if let Some(last) = line.last_mut() {
                let kept = last.text.trim_end().len();
                last.text.truncate(kept);
            }
            lines.push(line);
        }
    };
    row_lines((0..columns).map(|index| bold(cell(&table.head, index))).collect(), &mut lines);
    let rule = widths.iter().map(|width| "─".repeat(*width as usize)).collect::<Vec<_>>().join("  ");
    lines.push(vec![Span::new(rule, faint)]);
    for row in &table.rows {
        row_lines((0..columns).map(|index| cell(row, index)).collect(), &mut lines);
    }
    lines
}

/// A table too wide for its columns, a row at a time: each cell after its head, a blank line between rows.
fn table_by_rows(table: &Table, width: i32, faint: &Rc<Style>) -> Vec<Vec<Span>> {
    let mut lines = vec![];
    for (index, row) in table.rows.iter().enumerate() {
        if index > 0 {
            lines.push(vec![]);
        }
        for (column, cell) in row.iter().enumerate() {
            let head =
                table.head.get(column).map(|head| head.iter().map(|span| span.text.as_str()).collect::<String>()).unwrap_or_default();
            let spans: Vec<Span> = [Span::new(format!("{head}: "), faint)].into_iter().chain(cell.iter().cloned()).collect();
            lines.extend(wrap(&spans, width.max(1)));
        }
    }
    lines
}

/// Whether a cell reads as a number (a gain, a frequency, a share): a sign, digits, and at most a short unit after.
fn numeric(cell: &str) -> bool {
    let cell = cell.trim().trim_start_matches(['-', '+', '−', '±', '~']);
    let digits = cell.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).unwrap_or(cell.len());
    let unit = cell[digits..].trim();
    cell.starts_with(|c: char| c.is_ascii_digit())
        && unit.chars().count() <= 4
        && unit.chars().all(|c| c.is_alphabetic() || c == '%' || c == '×')
}
