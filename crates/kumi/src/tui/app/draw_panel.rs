use super::super::{
    picker::NoteTone,
    render::Cursor,
    screen::Screen,
    style::Style,
    width::{text_width, truncate},
    wrap::{wrap, Span},
};
use super::drawing::{sp, st};
use super::*;
/// A row of a panel's box: its words in a style, or spans of their own, with what's at the right edge.
pub(super) struct Line {
    pub text: String,
    pub style: Style,
    pub detail: Option<Span>,
    pub right: Option<Span>,
    pub band: bool,
    pub spans: Option<Vec<Span>>,
    pub indent: i32,
    pub label_width: i32,
}
impl Line {
    pub(super) fn new(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style, detail: None, right: None, band: false, spans: None, indent: 0, label_width: 0 }
    }
}
/// `text` wrapped to `width`, as lines in `style`.
pub(super) fn say(lines: &mut Vec<Line>, text: &str, style: Style, width: i32) {
    for row in wrap(&[sp(text, style)], width) {
        let text = row.into_iter().map(|s| s.text).collect::<String>();
        lines.push(Line::new(string::trim_end(&text), style));
    }
}
impl TuiApp {
    /// A list to choose from, in its box: its title (with its tabs, or what's typed to filter it), then the rows that
    /// fit around the one chosen.
    pub(super) fn draw_pick(&self, screen: &mut Screen, area: Area, picker: &Picker) -> Option<Cursor> {
        let width = 76.min(area.left - 2);
        let space = area.box_top - 5;
        let mut lines: Vec<Line> = vec![];
        let visible = picker.visible();
        let selected = picker.selected();
        let filter = if !picker.filter.is_empty() {
            Some(format!("filter: {}", picker.filter))
        } else if picker.options.filterable {
            Some("type to filter".into())
        } else {
            None
        };
        let mut title = Line::new(&picker.title, st::TITLE);
        title.right = filter.map(|f| sp(f, if picker.filter.is_empty() { st::FAINT } else { st::BRIGHT }));
        // Its tabs beside the title, the one showing lit.
        if let Some(tabs) = &picker.tabs {
            let mut spans = vec![sp(&picker.title, st::TITLE), sp("   ", st::FAINT)];
            for (at, name) in tabs.names.iter().enumerate() {
                if at > 0 {
                    spans.push(sp(" · ", st::FAINT));
                }
                spans.push(sp(name, if at == tabs.at { st::ACCENT } else { st::FAINT }));
            }
            title.spans = Some(spans);
        }
        lines.push(title);
        if visible.is_empty() {
            lines.push(Line::new(format!("Nothing matches “{}”.", picker.filter), st::FAINT));
        }
        let rows = (space - 2).clamp(1, 14);
        let at = selected.and_then(|s| visible.iter().position(|i| std::ptr::eq(*i, s))).unwrap_or(0) as i32;
        let mut first = (at - rows / 2).min(visible.len() as i32 - rows).max(0) as usize;
        if first > 0 && visible[first - 1].heading && at - (first as i32) < rows - 1 {
            first -= 1;
        }
        let shown = &visible[first..(first + rows as usize).min(visible.len())];
        let label_width = shown
            .iter()
            .filter(|i| !i.heading && i.detail.as_ref().is_some_and(|s| !s.is_empty()))
            .map(|i| text_width(&i.label))
            .max()
            .unwrap_or(0)
            .min(28);
        for (index, item) in shown.iter().enumerate() {
            let note = item.note.as_ref().filter(|s| !s.is_empty()).map(|s| {
                sp(
                    s,
                    match item.note_tone {
                        Some(NoteTone::Accent) => st::ACCENT,
                        Some(NoteTone::Warn) => st::WARN,
                        _ => st::FAINT,
                    },
                )
            });
            let more = if index == 0 && first > 0 {
                Some(format!("↑ {first} more"))
            } else if index == shown.len() - 1 && first + (rows as usize) < visible.len() {
                Some(format!("↓ {} more", visible.len() - first - rows as usize))
            } else {
                None
            };
            let right = note.or_else(|| more.map(|s| sp(s, st::FAINT)));
            if item.heading {
                let mut line = Line::new(&item.label, st::LABEL);
                line.right = right;
                lines.push(line);
                continue;
            }
            let chosen = selected.is_some_and(|s| std::ptr::eq(*item, s));
            let mut line = Line::new(
                &item.label,
                if item.inert {
                    st::FAINT
                } else if chosen {
                    st::ACCENT
                } else {
                    st::TEXT
                },
            );
            line.indent = 2;
            line.band = chosen;
            line.label_width = label_width;
            line.detail = item.detail.as_ref().filter(|s| !s.is_empty()).map(|s| sp(s, if chosen { st::BRIGHT } else { st::DIM }));
            line.right = right;
            lines.push(line);
        }
        self.draw_lines(screen, area.box_top, width, &lines, None)
    }
    /// A panel's `lines` in a box `width` wide over the conversation, down to the input box, whose top row is `box_top`;
    /// the cursor where `cursor_at` puts it, when that line shows.
    pub(super) fn draw_lines(
        &self,
        screen: &mut Screen,
        box_top: i32,
        width: i32,
        lines: &[Line],
        cursor_at: Option<(usize, i32)>,
    ) -> Option<Cursor> {
        let x = 3;
        let inner = width - 4;
        let gap = i32::from(lines.len() as i32 + 3 <= box_top - 2);
        let height = lines.len() as i32 + gap + 2;
        let top = (box_top - 1 - height).max(1);
        let area = Rect::new(1, top, width, height.min(box_top - 1 - top));
        screen.fill(area, &st::RAISED);
        self.cover(area);
        let row_of = |index: usize| top + 1 + index as i32 + if index > 0 { gap } else { 0 };
        for (index, line) in lines.iter().enumerate() {
            let y = row_of(index);
            if y >= box_top - 1 {
                continue;
            }
            if line.band {
                screen.fill(Rect::new(1, y, width, 1), &st::SELECTED);
            }
            let start = x + line.indent;
            let end = x + inner - line.right.as_ref().map(|r| text_width(&r.text) + 2).unwrap_or(0);
            let mut column = if let Some(spans) = &line.spans {
                super::drawing::spans(screen, start, y, spans, Some(Rect::new(start, y, (end - start).max(1), 1)))
            } else {
                screen.put(start, y, &truncate(&line.text, (end - start).max(1)), &line.style)
            };
            if let Some(detail) = &line.detail {
                column = column.max(start + line.label_width) + 2;
                if column < end {
                    screen.put(column, y, &truncate(&detail.text, end - column), &detail.style);
                }
            }
            if let Some(right) = &line.right {
                screen.put(x + inner - text_width(&right.text), y, &right.text, &right.style);
            }
        }
        cursor_at.filter(|(line, _)| row_of(*line) < box_top - 1).map(|(line, column)| Cursor { x: x + column, y: row_of(line) })
    }
}
