//! What Kumi is doing, as motion kept small: each kind of task has its own one-cell glyph, so a step at
//! work reads as searching, reading, building or listening at a glance, and a shimmer that passes over
//! the words of a step under way.

use kumi_common::js;

use super::style::{palette, Rgb, Style};
use super::wrap::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Activity {
    Think,
    Search,
    Read,
    Look,
    Build,
    Change,
    Listen,
    Watch,
    Play,
    Record,
    Code,
}

impl Activity {
    pub const ALL: [Activity; 11] = [
        Activity::Think,
        Activity::Search,
        Activity::Read,
        Activity::Look,
        Activity::Build,
        Activity::Change,
        Activity::Listen,
        Activity::Watch,
        Activity::Play,
        Activity::Record,
        Activity::Code,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Activity::Think => "think",
            Activity::Search => "search",
            Activity::Read => "read",
            Activity::Look => "look",
            Activity::Build => "build",
            Activity::Change => "change",
            Activity::Listen => "listen",
            Activity::Watch => "watch",
            Activity::Play => "play",
            Activity::Record => "record",
            Activity::Code => "code",
        }
    }
}

/// The kind of task a tool is, as Kumi and the DAW show it; the rest change the project.
pub fn activity_of(tool: Option<&str>) -> Activity {
    use kumi_agent::core::presentation::{activity, ToolActivity};
    let Some(tool) = tool.filter(|tool| !tool.is_empty()) else { return Activity::Think };
    match activity(crate::daw::presentation(), crate::daw::profile(), tool) {
        ToolActivity::Search => Activity::Search,
        ToolActivity::Read => Activity::Read,
        ToolActivity::Look => Activity::Look,
        ToolActivity::Build => Activity::Build,
        ToolActivity::Change => Activity::Change,
        ToolActivity::Listen => Activity::Listen,
        ToolActivity::Watch => Activity::Watch,
        ToolActivity::Play => Activity::Play,
        ToolActivity::Record => Activity::Record,
        ToolActivity::Code => Activity::Code,
    }
}

/// Each kind's one-cell frames, shown about ten a second.
fn glyphs(kind: Activity) -> &'static [&'static str] {
    match kind {
        // Dots turning: thinking it over.
        Activity::Think => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        // A dot circling, like a radar's sweep.
        Activity::Search => &["⠁", "⠈", "⠐", "⠠", "⢀", "⡀", "⠄", "⠂"],
        // Eyes going down a page, line by line.
        Activity::Read => &["⠉", "⠒", "⠤", "⣀", "⣀", "⠀"],
        // A gap going round: looking all over the Set.
        Activity::Look => &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"],
        // Filling up from the bottom: something being built.
        Activity::Build => &["⡀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣷", "⣿", "⣿", "⠀"],
        // A fader going up and down.
        Activity::Change => &["⣀", "⠤", "⠒", "⠉", "⠒", "⠤"],
        // A level meter.
        Activity::Listen => &["▁", "▄", "▂", "▆", "▃", "▇", "▅", "▂"],
        // A reel turning.
        Activity::Watch => &["◐", "◓", "◑", "◒"],
        Activity::Play => &["▶", "▶", "▷", "▷"],
        Activity::Record => &["●", "●", "○", "○"],
        // A block going round: code running.
        Activity::Code => &["▖", "▘", "▝", "▗"],
    }
}

/// The same, in characters every console font has (the old Windows console, where Kumi's icons are badges too).
fn plain_glyphs(kind: Activity) -> &'static [&'static str] {
    match kind {
        Activity::Think => &["|", "/", "-", "\\"],
        Activity::Search => &[".", "o", "O", "o"],
        Activity::Read => &["-", "=", "-", " "],
        Activity::Look => &["<", "^", ">", "v"],
        Activity::Build => &[".", ":", "|", "#", " "],
        Activity::Change => &["-", "=", "+", "="],
        Activity::Listen => &[".", ":", "|", ":"],
        Activity::Watch => &["o", "O"],
        Activity::Play => &[">", " "],
        Activity::Record => &["*", " "],
        Activity::Code => &["_", " "],
    }
}

const FRAME_MS: f64 = 100.0;

/// `Math.floor(value) % count` for a non-negative value, as JavaScript does it.
fn frame(value: f64, count: usize) -> usize {
    ((value.floor() as i64).rem_euclid(count as i64)) as usize
}

/// The glyph for `kind` at `ms` into it, and its style (recording's in red); `plain` for consoles without the glyphs.
pub fn activity_glyph(kind: Activity, ms: f64, plain: bool) -> Span {
    let frames = if plain { plain_glyphs(kind) } else { glyphs(kind) };
    let text = frames[frame(ms.max(0.0) / FRAME_MS, frames.len())];
    Span::styled(text, Style::fg(if kind == Activity::Record { palette::ERROR } else { palette::ACCENT }))
}

/// `from` toward `to` by `amount` (0–1), in six steps so the terminal's style table stays small.
pub fn blend(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    blend_steps(from, to, amount, 6)
}

/// `blend` in `steps` steps, between the theme's colours for `from` and `to`.
pub fn blend_steps(from: Rgb, to: Rgb, amount: f64, steps: u32) -> Rgb {
    let theme = super::style::theme();
    let (from, to) = (theme.of(from), theme.of(to));
    let step = js::number::round(amount.clamp(0.0, 1.0) * steps as f64) / steps as f64;
    let channel = |index: usize| js::number::round(from[index] as f64 + (to[index] as f64 - from[index] as f64) * step) as u8;
    [channel(0), channel(1), channel(2)]
}

/// Spans of one character each into as few spans as their styles allow.
fn joined(cells: Vec<Span>) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for cell in cells {
        match spans.last_mut() {
            Some(last) if same(&last.style, &cell.style) => last.text.push_str(&cell.text),
            _ => spans.push(Span::styled(cell.text, *cell.style)),
        }
    }
    spans
}

fn same(a: &Style, b: &Style) -> bool {
    a.fg == b.fg && a.bg == b.bg && a.bold == b.bold
}

/// `text` with a band of light passing over it, left to right, every couple of seconds: the words
/// of what's under way, so they never sit still while Kumi works.
pub fn shimmer(text: &str, ms: f64) -> Vec<Span> {
    shimmer_with(text, ms, palette::DIM, palette::BRIGHT)
}

/// `shimmer` between colours of one's own.
pub fn shimmer_with(text: &str, ms: f64, base: Rgb, peak: Rgb) -> Vec<Span> {
    let characters: Vec<char> = text.chars().collect();
    let span = characters.len() as f64 + 8.0;
    let center = ((ms / 1800.0) % 1.0) * span - 4.0;
    joined(
        characters
            .iter()
            .enumerate()
            .map(|(index, character)| {
                let distance = (index as f64 - center).abs();
                Span::styled(character.to_string(), Style::fg(blend(base, peak, (1.0 - distance / 3.5).max(0.0))))
            })
            .collect(),
    )
}
