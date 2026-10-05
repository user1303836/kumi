//! Conversation rows and timed repeated-step folding.
use super::{
    activity::{activity_of, blend_steps, Activity},
    markdown::render_markdown,
    style::{palette, Rgb, Style},
    width::text_width,
    wrap::{wrap, Span},
};
use kumi_common::js::number;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Deref,
    rc::Rc,
};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: String,
    pub tool: Option<String>,
    pub label: String,
    pub state: StepState,
    pub ms: Option<f64>,
    pub doing: Option<String>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub folded: Option<f64>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Running,
    #[default]
    Done,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Note,
    Technique,
    Recipe,
    Lesson,
}
impl MemoryKind {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Note => "✎",
            Self::Technique => "◆",
            Self::Recipe => "↻",
            Self::Lesson => "✦",
        }
    }
    pub fn color(self) -> Rgb {
        match self {
            Self::Note => palette::NOTE,
            Self::Technique => palette::TECHNIQUE,
            Self::Recipe => palette::RECIPE,
            Self::Lesson => palette::LESSON,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comparison {
    pub reference: String,
    pub summary: String,
    pub differences: Vec<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Best {
    pub label: String,
    pub score: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Take {
    pub label: String,
    pub score: Option<f64>,
    pub silent: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub at: f64,
    pub zoom: Option<String>,
    pub thumb: Picture,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sound {
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebLine {
    pub lead: String,
    pub title: String,
    pub detail: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerStatus {
    Running,
    Done,
    Stopped,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeTone {
    Info,
    Warn,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    User {
        text: String,
    },
    Assistant {
        text: String,
        steps: Vec<Step>,
        status: AnswerStatus,
        #[serde(rename = "elapsedMs")]
        elapsed_ms: Option<f64>,
        #[serde(rename = "startedAt")]
        started_at: Option<f64>,
    },
    Notice {
        text: String,
        tone: NoticeTone,
    },
    Divider {
        text: String,
    },
    Memory {
        what: MemoryKind,
        text: String,
    },
    Heard {
        file: String,
        summary: String,
        bands: Vec<f64>,
        compared: Option<Comparison>,
    },
    Auditioned {
        round: f64,
        best: Option<Best>,
        previous: Option<f64>,
        takes: Vec<Take>,
        gaps: Vec<String>,
    },
    Watched {
        title: String,
        channel: Option<String>,
        duration: Option<f64>,
        from: f64,
        to: f64,
        chapters: Vec<String>,
        words: String,
        frames: Vec<Frame>,
        sound: Option<Sound>,
        notes: Vec<String>,
        pictures: bool,
    },
    Web {
        lines: Vec<WebLine>,
    },
}
#[derive(Clone, Debug)]
pub struct Band {
    pub bg: Rgb,
    pub width: i32,
}
#[derive(Clone, Debug)]
pub enum LiveRow {
    Step { activity: Activity, since: f64, label: String, doing: Option<String> },
    Header { label: String, since: Option<f64> },
}
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub spans: Vec<Span>,
    pub band: Option<Band>,
    pub trailing: Option<Span>,
    pub live: Option<LiveRow>,
}
thread_local! {static STYLES:RefCell<HashMap<Rgb,Rc<Style>>>=RefCell::new(HashMap::new());}
fn style(color: Rgb) -> Rc<Style> {
    STYLES.with(|styles| styles.borrow_mut().entry(color).or_insert_with(|| Rc::new(Style::fg(color))).clone())
}
fn span(text: impl Into<String>, color: Rgb) -> Span {
    Span::new(text, &style(color))
}
fn row(spans: Vec<Span>) -> Row {
    Row { spans, ..Default::default() }
}
fn wrapped(spans: &[Span], width: i32) -> Vec<Row> {
    wrap(spans, width).into_iter().map(row).collect()
}
fn seconds(ms: f64) -> String {
    format!("{}s", number::to_fixed(ms / 1000., 1))
}
pub const FOLD_AFTER_MS: f64 = 3000.;
const FOLD_FADE_MS: f64 = 220.;
fn fold_span(extra: usize) -> f64 {
    FOLD_FADE_MS + extra as f64 * 70f64.min(380. / extra.max(1) as f64)
}
fn alike(a: &Step, b: &Step) -> bool {
    a.label == b.label && a.state == b.state && a.state != StepState::Running
}
pub fn fold_steps(steps: &mut [Step], now: f64) -> bool {
    let mut changed = false;
    let mut start = 0;
    while start < steps.len() {
        let mut end = start + 1;
        while end < steps.len() && alike(&steps[start], &steps[end]) {
            end += 1;
        }
        let waiting: Vec<usize> = (start + 1..end).filter(|i| steps[*i].folded.is_none()).collect();
        if !waiting.is_empty() {
            let due = std::iter::once(start)
                .chain(waiting.iter().copied())
                .map(|i| steps[i].ended_at.unwrap_or(f64::NEG_INFINITY))
                .fold(f64::NEG_INFINITY, f64::max)
                + FOLD_AFTER_MS;
            if now >= due {
                let folded = if due.is_finite() { due } else { now - fold_span(waiting.len()) - 1. };
                for i in waiting {
                    steps[i].folded = Some(folded);
                }
                changed = true;
            }
        }
        start = end;
    }
    changed
}
pub fn steps_change_at(steps: &[Step], now: f64) -> Option<f64> {
    let mut soonest: Option<f64> = None;
    let mut consider = |at: f64| soonest = Some(soonest.map_or(at, |s| s.min(at)));
    let mut start = 0;
    while start < steps.len() {
        let mut end = start + 1;
        while end < steps.len() && alike(&steps[start], &steps[end]) {
            end += 1;
        }
        let members = &steps[start + 1..end];
        let waiting: Vec<_> = members.iter().filter(|step| step.folded.is_none()).collect();
        if !waiting.is_empty() {
            consider(now.max(
                std::iter::once(&steps[start]).chain(waiting).map(|step| step.ended_at.unwrap_or(0.)).fold(f64::NEG_INFINITY, f64::max)
                    + FOLD_AFTER_MS,
            ));
        }
        for step in members {
            if step.folded.is_some_and(|at| now < at + fold_span(members.len()) + 1.) {
                consider(now);
            }
        }
        start = end;
    }
    soonest
}
fn step_row(step: &Step, fade: f64, folded: Option<(usize, f64)>) -> Row {
    let faded = |text: String, color: Rgb| {
        if fade > 0. {
            Span::styled(text, Style::fg(blend_steps(color, palette::GROUND, fade, 5)))
        } else {
            span(text, color)
        }
    };
    let (glyph, color) = if step.state == StepState::Error { ("×", palette::ERROR) } else { ("✓", palette::ACCENT) };
    let mut spans = vec![span("│ ", palette::RULE), faded(glyph.into(), color), faded(format!(" {}", step.label), palette::DIM)];
    if let Some((count, _)) = folded {
        spans.push(span(format!(" ×{count}"), palette::FAINT));
    }
    Row { spans, trailing: step.ms.map(|ms| faded(seconds(folded.map_or(ms, |(_, ms)| ms)), palette::FAINT)), ..Default::default() }
}
fn step_rows(steps: &[Step], now: f64) -> Vec<Row> {
    let mut rows = vec![];
    let mut start = 0;
    while start < steps.len() {
        let first = &steps[start];
        if first.state == StepState::Running {
            let label = doing_label(first.tool.as_deref(), &first.label);
            rows.push(Row {
                spans: vec![span("│ ", palette::RULE), span(" ", palette::ACCENT), span(format!(" {label}"), palette::DIM)],
                live: Some(LiveRow::Step {
                    activity: activity_of(first.tool.as_deref()),
                    since: first.started_at.unwrap_or(now),
                    label,
                    doing: first.doing.clone().filter(|s| !s.is_empty()),
                }),
                ..Default::default()
            });
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < steps.len() && alike(first, &steps[end]) {
            end += 1;
        }
        let members = &steps[start + 1..end];
        let duration = fold_span(members.len());
        let mut fading = vec![];
        let mut gone = 0;
        let mut gone_ms = 0.;
        for (index, step) in members.iter().enumerate() {
            if let Some(folded) = step.folded {
                let into = now - folded;
                let leaves = FOLD_FADE_MS + (members.len() - 1 - index) as f64 * ((duration - FOLD_FADE_MS) / members.len().max(1) as f64);
                if into >= leaves {
                    gone += 1;
                    gone_ms += step.ms.unwrap_or(0.);
                } else {
                    fading.push((step, (into / FOLD_FADE_MS).clamp(0., 1.)));
                }
            } else {
                fading.push((step, 0.));
            }
        }
        rows.push(step_row(first, 0., (gone > 0).then_some((gone + 1, first.ms.unwrap_or(0.) + gone_ms))));
        for (step, fade) in fading {
            rows.push(step_row(step, fade, None));
        }
        start = end;
    }
    rows
}
pub fn entry_rows(entry: &Entry, width: i32, now: f64) -> Vec<Row> {
    let inner = width.max(1);
    match entry {
        Entry::User { text } => {
            let lines = wrap(&[span(text, palette::BRIGHT)], inner);
            let band =
                lines.iter().map(|line| text_width(&line.iter().map(|s| s.text.as_str()).collect::<String>())).max().unwrap_or(0) + 2;
            lines
                .into_iter()
                .map(|spans| Row { spans, band: Some(Band { bg: palette::RAISED, width: band }), ..Default::default() })
                .collect()
        }
        Entry::Notice { text, tone } => {
            wrapped(&[span(text, if *tone == NoticeTone::Warn { palette::WARN } else { palette::FAINT })], inner)
        }
        Entry::Memory { what, text } => wrap(&[span(text, palette::DIM)], (inner - 2).max(1))
            .into_iter()
            .enumerate()
            .map(|(i, mut spans)| {
                spans.insert(0, if i == 0 { span(format!("{} ", what.glyph()), what.color()) } else { span("  ", palette::DIM) });
                row(spans)
            })
            .collect(),
        Entry::Divider { text } => wrapped(
            &[
                span("── ", palette::RULE),
                span(text, palette::DIM),
                span(format!(" {}", "─".repeat((inner - text_width(text) - 4).max(2) as usize)), palette::RULE),
            ],
            inner,
        ),
        Entry::Heard { file, summary, bands, compared } => heard_rows(file, summary, bands, compared.as_ref(), inner),
        Entry::Auditioned { round, best, previous, takes, gaps } => auditioned_rows(*round, best.as_ref(), *previous, takes, gaps, inner),
        Entry::Watched { .. } => watched_rows(entry, inner),
        Entry::Web { lines } => lines
            .iter()
            .flat_map(|line| {
                let mut spans = vec![span(format!("{} ", line.lead), palette::DIM), span(&line.title, palette::TEXT)];
                if !line.detail.is_empty() {
                    spans.push(span(format!(" · {}", line.detail), palette::FAINT));
                }
                wrapped(&spans, inner)
            })
            .collect(),
        Entry::Assistant { text, steps, status, elapsed_ms, started_at } => {
            let mut rows = vec![];
            if !text.is_empty() {
                for r in render_markdown(text.trim_end_matches('\n'), inner, &style(palette::TEXT)) {
                    rows.push(Row { spans: r.spans, band: r.bg.map(|bg| Band { bg, width: inner + 2 }), ..Default::default() });
                }
            }
            if !steps.is_empty() {
                if !rows.is_empty() {
                    rows.push(Row::default());
                }
                if *status == AnswerStatus::Running {
                    rows.push(Row {
                        spans: vec![span("▾ ", palette::FAINT), span("working", palette::DIM)],
                        live: Some(LiveRow::Header { label: "working".into(), since: *started_at }),
                        ..Default::default()
                    });
                } else {
                    let mut spans = vec![
                        span("▾ ", palette::FAINT),
                        span(format!("{} {}", steps.len(), if steps.len() == 1 { "step" } else { "steps" }), palette::DIM),
                    ];
                    if let Some(ms) = elapsed_ms {
                        spans.push(span(format!(" · {}", seconds(*ms)), palette::FAINT));
                    }
                    rows.push(row(spans));
                }
                rows.extend(step_rows(steps, now));
            }
            if *status == AnswerStatus::Stopped {
                rows.push(row(vec![span("stopped", palette::FAINT)]));
            }
            if *status == AnswerStatus::Failed && text.is_empty() {
                rows.push(row(vec![span("Kumi couldn't answer that; see the note below.", palette::FAINT)]));
            }
            rows
        }
    }
}
const BAND_LABELS: [&str; 10] = ["sub", "bass", "u.bas", "l.mid", "mids", "u.mid", "pres", "bite", "brill", "air"];
const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
fn pad(text: &str, cells: usize) -> String {
    let mut units: Vec<_> = text.encode_utf16().collect();
    units.resize(cells, 32);
    units.truncate(cells);
    String::from_utf16_lossy(&units)
}
fn heard_rows(file: &str, summary: &str, bands: &[f64], compared: Option<&Comparison>, width: i32) -> Vec<Row> {
    let cell = if width >= 60 { 6 } else { 5 };
    let fits = (width / cell).clamp(1, 10) as usize;
    let title =
        compared.map_or_else(|| format!("Heard {file} · {summary}"), |c| format!("Heard {file} against {}, loudness matched", c.reference));
    let mut rows = wrapped(&[span(title, palette::DIM)], width);
    rows.push(row(if let Some(c) = compared {
        c.differences
            .iter()
            .take(fits)
            .map(|value| {
                span(
                    pad(
                        &format!(
                            "{}{}",
                            if *value > 0. {
                                "+"
                            } else if *value < 0. {
                                "−"
                            } else {
                                " "
                            },
                            number::to_fixed(value.abs(), 1)
                        ),
                        cell as usize,
                    ),
                    if value.abs() >= 1.5 {
                        if *value > 0. {
                            palette::WARN
                        } else {
                            palette::ACCENT
                        }
                    } else {
                        palette::FAINT
                    },
                )
            })
            .collect()
    } else {
        let loudest = bands.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        bands
            .iter()
            .take(fits)
            .map(|value| {
                let level = number::round(7. + (value - loudest) / 4.).clamp(0., 7.) as usize;
                span(pad(&BARS[level].repeat(cell as usize - 2), cell as usize), palette::ACCENT)
            })
            .collect()
    }));
    rows.push(row(BAND_LABELS.iter().take(fits).map(|label| span(pad(label, cell as usize), palette::FAINT)).collect()));
    rows
}
fn auditioned_rows(round: f64, best: Option<&Best>, previous: Option<f64>, takes: &[Take], gaps: &[String], width: i32) -> Vec<Row> {
    let mut spans = vec![span(format!("Round {} · ", number::to_string(round)), palette::DIM)];
    if let Some(best) = best {
        if let Some(previous) = previous {
            spans.push(span(format!("{}% → ", number::to_string(previous)), palette::DIM));
        }
        spans.push(span(
            format!("{}%", number::to_string(best.score)),
            if previous.is_some_and(|p| best.score < p) { palette::WARN } else { palette::ACCENT },
        ));
        if !gaps.is_empty() {
            spans.push(span(format!(" · {}", gaps.join(", ")), palette::DIM));
        }
    } else {
        spans.push(span(
            if takes.iter().all(|take| take.silent == Some(true)) { "the render was silent" } else { "listened" },
            palette::DIM,
        ));
    }
    let mut rows = wrapped(&spans, width);
    if takes.len() > 1 {
        rows.extend(wrapped(
            &[span(
                takes
                    .iter()
                    .map(|take| {
                        format!(
                            "{} {}",
                            take.label,
                            if take.silent == Some(true) {
                                "silent".into()
                            } else {
                                take.score.map(number::to_string).unwrap_or("–".into())
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
                palette::FAINT,
            )],
            width,
        ));
    }
    rows
}
fn clock(seconds: f64) -> String {
    let whole = seconds.floor().max(0.) as u64;
    let hours = whole / 3600;
    let minutes = whole % 3600 / 60;
    let secs = whole % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}
fn area(picture: &Picture, x0: f64, y0: f64, x1: f64, y1: f64) -> Rgb {
    let mut sum = [0u64; 3];
    let mut count = 0;
    for y in y0.floor() as usize..y1.ceil().max(y0.floor() + 1.) as usize {
        for x in x0.floor() as usize..x1.ceil().max(x0.floor() + 1.) as usize {
            let at = (y.min(picture.height.saturating_sub(1)) * picture.width + x.min(picture.width.saturating_sub(1))) * 3;
            for c in 0..3 {
                sum[c] += picture.rgb.get(at + c).copied().unwrap_or(0) as u64;
            }
            count += 1;
        }
    }
    sum.map(|n| number::round(n as f64 / count as f64) as u8)
}
pub fn picture_rows(picture: &Picture, cells: usize) -> Vec<Vec<Span>> {
    let height = (number::round(cells as f64 * picture.height as f64 / picture.width as f64 / 2.) * 2.).max(2.) as usize;
    let across = picture.width as f64 / cells as f64;
    let down = picture.height as f64 / height as f64;
    (0..height / 2)
        .map(|row| {
            (0..cells)
                .map(|cell| {
                    Span::styled(
                        "▀",
                        Style {
                            fg: Some(area(
                                picture,
                                cell as f64 * across,
                                (row * 2) as f64 * down,
                                (cell + 1) as f64 * across,
                                (row * 2 + 1) as f64 * down,
                            )),
                            bg: Some(area(
                                picture,
                                cell as f64 * across,
                                (row * 2 + 1) as f64 * down,
                                (cell + 1) as f64 * across,
                                (row * 2 + 2) as f64 * down,
                            )),
                            ..Default::default()
                        },
                    )
                })
                .collect()
        })
        .collect()
}
fn watched_rows(entry: &Entry, width: i32) -> Vec<Row> {
    let Entry::Watched { title, channel, duration, from, to, chapters, words, frames, sound, notes, pictures } = entry else {
        unreachable!()
    };
    let mut rows = wrapped(
        &[
            span("Watched ", palette::DIM),
            span(format!("“{title}”"), palette::TEXT),
            span(
                format!(
                    "{}{}",
                    channel.as_ref().filter(|c| !c.is_empty()).map(|c| format!(" · {c}")).unwrap_or_default(),
                    duration.filter(|d| *d != 0.).map(|d| format!(" · {}", clock(d))).unwrap_or_default()
                ),
                palette::DIM,
            ),
        ],
        width,
    );
    let watched = if duration.is_some_and(|d| d != 0. && *from <= 0. && *to >= d - 1.) {
        "the whole video".into()
    } else {
        format!("{}–{}", clock(*from), clock(*to))
    };
    rows.extend(wrapped(
        &[span(
            format!(
                "{watched} · {words}{}",
                sound.as_ref().map(|s| format!(" · kept its sound at {}–{}", clock(s.from), clock(s.to))).unwrap_or_default()
            ),
            palette::FAINT,
        )],
        width,
    ));
    if !chapters.is_empty() {
        rows.extend(wrapped(&[span(format!("chapters: {}", chapters.join(" · ")), palette::FAINT)], width));
    }
    let times = |frame: &Frame| {
        format!("{}{}", clock(frame.at), frame.zoom.as_ref().filter(|s| !s.is_empty()).map(|s| format!(" {s}")).unwrap_or_default())
    };
    if !frames.is_empty() && *pictures && width >= 24 {
        let cells = ((width + 1) / 4 - 1).clamp(10, 24) as usize;
        let across = ((width + 1) as usize / (cells + 1)).max(1);
        let shown = &frames[..frames.len().min(8)];
        for group in shown.chunks(across) {
            let pics: Vec<_> = group.iter().map(|frame| picture_rows(&frame.thumb, cells)).collect();
            let height = pics.iter().map(Vec::len).max().unwrap_or(0);
            for line in 0..height {
                let mut spans = vec![];
                for (index, pic) in pics.iter().enumerate() {
                    if index > 0 {
                        spans.push(span(" ", palette::FAINT));
                    }
                    spans.extend(pic.get(line).cloned().unwrap_or_else(|| vec![span(" ".repeat(cells), palette::FAINT)]));
                }
                rows.push(row(spans));
            }
            rows.push(row(group
                .iter()
                .enumerate()
                .map(|(index, frame)| span(format!("{}{}", if index > 0 { " " } else { "" }, pad(&times(frame), cells)), palette::FAINT))
                .collect()));
        }
        if frames.len() > shown.len() {
            rows.push(row(vec![span(
                format!("and {}", frames[shown.len()..].iter().map(times).collect::<Vec<_>>().join(", ")),
                palette::FAINT,
            )]));
        }
    } else if !frames.is_empty() {
        rows.extend(wrapped(
            &[span(format!("looked at {}", frames.iter().map(times).collect::<Vec<_>>().join(", ")), palette::FAINT)],
            width,
        ));
    }
    for note in notes {
        rows.extend(wrapped(&[span(note, palette::FAINT)], width));
    }
    rows
}
#[derive(Clone)]
struct Cache {
    width: i32,
    revision: u64,
    rows: Vec<Row>,
    moving: bool,
}
pub struct EntryState {
    entry: RefCell<Entry>,
    revision: Cell<u64>,
    cache: RefCell<Option<Cache>>,
}
impl Deref for EntryState {
    type Target = RefCell<Entry>;
    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}
pub type EntryRef = Rc<EntryState>;
#[derive(Default)]
pub struct Transcript {
    pub entries: Vec<EntryRef>,
    pub laid_out: usize,
}
impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn add(&mut self, entry: Entry) -> EntryRef {
        self.insert_before(entry, None)
    }
    pub fn insert_before(&mut self, entry: Entry, before: Option<&EntryRef>) -> EntryRef {
        let entry = Rc::new(EntryState { entry: RefCell::new(entry), revision: Cell::new(0), cache: RefCell::new(None) });
        let index = before.and_then(|before| self.entries.iter().position(|e| Rc::ptr_eq(e, before))).unwrap_or(self.entries.len());
        self.entries.insert(index, entry.clone());
        entry
    }
    pub fn remove(&mut self, entry: &EntryRef) {
        if let Some(index) = self.entries.iter().position(|e| Rc::ptr_eq(e, entry)) {
            self.entries.remove(index);
        }
    }
    pub fn touch(&self, entry: &EntryRef) {
        entry.revision.set(entry.revision.get().wrapping_add(1));
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn change_at(&self, now: f64) -> Option<f64> {
        self.entries
            .iter()
            .filter_map(|e| {
                if let Entry::Assistant { steps, .. } = &*e.borrow() {
                    if steps.len() > 1 {
                        return steps_change_at(steps, now);
                    }
                }
                None
            })
            .reduce(f64::min)
    }
    pub fn rows(&mut self, width: i32, now: f64) -> Vec<Row> {
        let mut all = vec![];
        for entry in &self.entries {
            let mut moving = false;
            {
                if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
                    if steps.len() > 1 {
                        if fold_steps(steps, now) {
                            entry.revision.set(entry.revision.get().wrapping_add(1));
                        }
                        moving = steps_change_at(steps, now) == Some(now);
                    }
                }
            }
            let revision = entry.revision.get();
            let mut cached = entry.cache.borrow_mut();
            if cached.as_ref().is_none_or(|c| c.width != width || c.revision != revision || moving || c.moving) {
                self.laid_out += 1;
                *cached = Some(Cache { width, revision, rows: entry_rows(&entry.borrow(), width, now), moving });
            }
            if !all.is_empty() {
                all.push(Row::default());
            }
            all.extend(cached.as_ref().unwrap().rows.clone());
        }
        all
    }
}

pub fn step_label(tool: &str) -> String {
    match tool {
        "server_status" => "checked the Ableton bridge".into(),
        "live_status" => "checked Live".into(),
        "live_discover" => "looked at your Set".into(),
        "live_snapshot" => "read your whole Set".into(),
        "live_browser_search" => "searched the Browser".into(),
        "live_note_read" => "read notes".into(),
        "set_tempo" => "changed the tempo".into(),
        "set_mixer" => "changed the mixer".into(),
        "rename" => "renamed".into(),
        "add_tracks_and_scenes" => "added tracks or scenes".into(),
        "write_midi_clip" => "wrote a MIDI clip".into(),
        "load_device" => "loaded a device".into(),
        "set_device_parameter" => "moved a device control".into(),
        "set_locators" => "set locators".into(),
        "set_track_color" => "changed a track colour".into(),
        "undo_change" => "undid a change".into(),
        "run_python" => "ran Python in Live".into(),
        "make_changes" => "made changes".into(),
        "arrange" => "arranged".into(),
        "listen" => "listened".into(),
        "audition" => "listened to it quietly".into(),
        "run_recipe" => "ran a recipe".into(),
        "play" => "played or stopped".into(),
        "fire_scene" => "launched a scene".into(),
        "launch_clip" => "launched a clip".into(),
        "record" => "recorded".into(),
        "jump_to_locator" => "moved the playhead".into(),
        "select" => "showed you in Live".into(),
        "show" => "changed the view".into(),
        "set_transport" => "changed the transport".into(),
        "set_routing" => "changed routing".into(),
        "transform_midi" => "transformed notes".into(),
        "set_automation" => "drew automation".into(),
        "change_structure" => "changed tracks".into(),
        "set_song" => "changed song settings".into(),
        "find_sounds" => "looked for sounds".into(),
        "find_presets" => "looked for presets".into(),
        "my_sets" => "looked through your Sets".into(),
        "live_manual" => "read Live's manual".into(),
        "load_sample" => "loaded a sample".into(),
        "load_sample_to_pad" => "loaded a pad".into(),
        "edit_rack" => "edited a rack".into(),
        "set_chain_mixer" => "changed a rack chain".into(),
        "set_mixer_options" => "changed mixer options".into(),
        "set_clip" => "changed a clip".into(),
        "set_audio_clip" => "changed an audio clip".into(),
        "edit_clip" => "edited a clip".into(),
        "duplicate_clip" => "copied a clip".into(),
        "move_clip" => "moved a clip".into(),
        "add_arrangement_clip" => "added an Arrangement clip".into(),
        "change_notes" => "changed notes".into(),
        "delete_notes" => "deleted notes".into(),
        "edit_notes" => "edited notes".into(),
        "set_scene" => "changed a scene".into(),
        "capture_scene" => "captured a scene".into(),
        "switch_device" => "switched a device".into(),
        "move_device" => "moved a device".into(),
        "move_device_to" => "moved a device".into(),
        "delete_device" => "deleted a device".into(),
        "set_chain" => "changed a chain".into(),
        "set_scale" => "set the scale".into(),
        "set_groove" => "changed the groove".into(),
        "replace_sample" => "swapped a sample".into(),
        "import_audio" => "imported audio".into(),
        "set_warp_markers" => "moved warp markers".into(),
        "capture_midi" => "captured MIDI".into(),
        "set_device_details" => "changed device settings".into(),
        "use_looper" => "used the Looper".into(),
        "set_sidechain" => "set a sidechain".into(),
        "live_song_state" => "read the song's settings".into(),
        "live_performance_read" => "checked Live's load".into(),
        "live_key_estimate" => "estimated the key".into(),
        "live_take_lane_read" => "read take lanes".into(),
        "live_warp_marker_read" => "read warp markers".into(),
        "live_arrangement_automation_read" => "read automation".into(),
        "live_browser_roots" => "looked in the Browser".into(),
        "live_browser_inspect" => "looked in the Browser".into(),
        "watch_me" => "watched you work".into(),
        "save_recipe" => "saved a recipe".into(),
        "watch_video" => "watched a video".into(),
        "make_device" => "made a device".into(),
        "search_web" => "searched the web".into(),
        "read_web" => "read a page".into(),
        _ => tool.strip_prefix("live_").unwrap_or(tool).replace("_", " "),
    }
}

pub fn doing_label(tool: Option<&str>, fallback: &str) -> String {
    if tool == Some("") {
        return String::new();
    }
    match tool.unwrap_or("") {
        "live_status" => "checking Live".into(),
        "live_discover" => "looking at your Set".into(),
        "live_snapshot" => "reading your whole Set".into(),
        "live_browser_search" => "searching the Browser".into(),
        "live_note_read" => "reading notes".into(),
        "make_changes" => "making changes".into(),
        "arrange" => "arranging".into(),
        "listen" => "listening".into(),
        "audition" => "listening to it quietly".into(),
        "run_recipe" => "running a recipe".into(),
        "play" => "playing".into(),
        "record" => "recording".into(),
        "select" => "showing you".into(),
        "find_sounds" => "looking for sounds".into(),
        "find_presets" => "looking for presets".into(),
        "my_sets" => "looking through your Sets".into(),
        "live_manual" => "reading Live's manual".into(),
        "undo_change" => "undoing a change".into(),
        "run_python" => "running Python in Live".into(),
        "watch_me" => "comparing your Set".into(),
        "live_key_estimate" => "estimating the key".into(),
        "watch_video" => "watching the video".into(),
        "make_device" => "making a device".into(),
        "search_web" => "searching the web".into(),
        "read_web" => "reading a page".into(),
        _ => fallback.into(),
    }
}
