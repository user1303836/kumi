use super::super::{
    activity::{activity_glyph, Activity},
    markdown::render_markdown,
    picker::NoteTone,
    render::Cursor,
    screen::Screen,
    style::Style,
    width::{text_width, truncate},
    wrap::{wrap, Span},
};
use super::drawing::{sp, st};
use super::*;
use kumi_runtime::providers::provider_info;
struct Line {
    text: String,
    style: Style,
    detail: Option<Span>,
    right: Option<Span>,
    band: bool,
    spans: Option<Vec<Span>>,
    indent: i32,
    label_width: i32,
}
impl Line {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style, detail: None, right: None, band: false, spans: None, indent: 0, label_width: 0 }
    }
}
fn say(lines: &mut Vec<Line>, text: &str, style: Style, width: i32) {
    for row in wrap(&[sp(text, style)], width) {
        let text = row.into_iter().map(|s| s.text).collect::<String>();
        lines.push(Line::new(string::trim_end(&text), style));
    }
}
impl TuiApp {
    pub(super) fn draw_panel(&self, screen: &mut Screen, panel: &PanelRef, box_top: i32, left: i32) -> Option<Cursor> {
        let width = 76.min(left - 2);
        let x = 3;
        let inner = width - 4;
        let space = box_top - 5;
        let mut lines: Vec<Line> = vec![];
        let mut cursor_at: Option<(usize, i32)> = None;
        match &mut *panel.borrow_mut() {
            Panel::Pick { picker, .. } => {
                let picker = picker.borrow();
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
            }
            Panel::Btw { at, scroll } => {
                let (aside, len) = {
                    let state = self.0.state.borrow();
                    (state.asides.get(*at).cloned(), state.asides.len())
                };
                let mut title = Line::new(
                    format!("btw · {}", aside.as_ref().map(|a| a.borrow().question.replace('\n', " ")).unwrap_or_default()),
                    st::TITLE,
                );
                if len > 1 {
                    title.right = Some(sp(format!("{} of {len} · ←→", *at + 1), st::FAINT));
                }
                lines.push(title);
                if let Some(aside) = aside {
                    let aside = aside.borrow();
                    if string::trim(&aside.answer).is_empty() {
                        let mut line = Line::new("", st::DIM);
                        line.spans = Some(if aside.state == "asking" {
                            vec![activity_glyph(Activity::Think, perf_now(), false), sp(" thinking it over…", st::DIM)]
                        } else {
                            vec![sp("No answer came back.", st::FAINT)]
                        });
                        lines.push(line);
                    } else {
                        let rows = render_markdown(
                            string::trim(&aside.answer),
                            inner,
                            &Rc::new(if aside.state == "failed" { st::WARN } else { st::TEXT }),
                        );
                        let room = (space - 4).clamp(3, 16);
                        *scroll = (*scroll).min(rows.len() as i32 - room).max(0);
                        let shown = &rows[*scroll as usize..(*scroll as usize + room as usize).min(rows.len())];
                        for (index, row) in shown.iter().enumerate() {
                            let more = if index == 0 && *scroll > 0 {
                                Some(format!("↑ {} more", *scroll))
                            } else if index == shown.len() - 1 && *scroll + room < rows.len() as i32 {
                                Some(format!("↓ {} more", rows.len() as i32 - *scroll - room))
                            } else {
                                None
                            };
                            let mut line = Line::new("", st::TEXT);
                            line.spans = Some(row.spans.clone());
                            line.right = more.map(|s| sp(s, st::FAINT));
                            lines.push(line);
                        }
                        if aside.state == "asking" {
                            let mut line = Line::new("", st::DIM);
                            line.spans = Some(vec![activity_glyph(Activity::Think, perf_now(), false)]);
                            lines.push(line);
                        }
                    }
                    if aside.state == "asking" {
                        self.0.scheduler.request();
                    }
                }
            }
            Panel::Key { provider, secret, checking, status, .. } => {
                let info = provider_info(*provider);
                lines.push(Line::new(format!("Sign in to {}", info.name), st::TITLE));
                say(&mut lines, &format!("Paste your {} API key. It stays hidden, even here.", info.name), st::DIM, inner);
                let count = secret.encode_utf16().count();
                let dots = "•".repeat(count.min((inner - 20).max(8) as usize));
                let mut line = Line::new(&dots, st::BRIGHT);
                line.band = true;
                if !secret.is_empty() {
                    line.right = Some(sp(format!("{count} characters"), st::FAINT));
                }
                lines.push(line);
                if !*checking {
                    cursor_at = Some((lines.len() - 1, text_width(&dots)));
                }
                if let Some((text, tone)) = status {
                    say(&mut lines, text, if *tone == NoticeTone::Warn { st::WARN } else { st::DIM }, inner);
                } else if let Some(page) = info.key_page {
                    say(&mut lines, &format!("Make one at {page}."), st::FAINT, inner);
                }
                say(
                    &mut lines,
                    &format!("Kumi checks it with {}, then keeps it in ~/.kumi, readable only by you.", info.name),
                    st::FAINT,
                    inner,
                );
            }
            Panel::ChatGpt { url, .. } => {
                lines.push(Line::new("Sign in to ChatGPT", st::TITLE));
                if let Some(url) = url {
                    say(&mut lines, "Finish in your browser. If it didn't open, open this link (c copies it):", st::DIM, inner);
                    for part in helpers::chunk(url, inner.max(1) as usize).into_iter().take((space - 4).max(1) as usize) {
                        lines.push(Line::new(part, st::ACCENT));
                    }
                    lines.push(Line::new("Waiting for the browser…", st::FAINT));
                } else {
                    say(&mut lines, "Starting the sign-in…", st::DIM, inner);
                }
            }
        }
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
